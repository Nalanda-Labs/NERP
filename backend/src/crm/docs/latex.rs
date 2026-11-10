//! LuaLaTeX rendering (external `lualatex`).
//!
//! Data never gets string-pasted into TeX. It is written to `data.lua` as a Lua table
//! literal, and templates print values with `\crm{path.to.value}`, which uses
//! `tex.sprint(-2, …)`: catcode −2 typesets every character literally, so a customer
//! named `\input{/etc/passwd}` or `100% {x}` is printed, never executed.

use serde_json::Value;

use super::process::{run, RunError};

/// Serialises JSON as a Lua table literal with safe string escaping.
pub fn to_lua(v: &Value) -> String {
    let mut out = String::new();
    write_lua(v, &mut out);
    out
}

fn lua_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\{:03}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_lua(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("\"\""),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => lua_str(&n.to_string(), out),
        Value::String(s) => lua_str(s, out),
        Value::Array(a) => {
            out.push('{');
            for x in a {
                write_lua(x, out);
                out.push(',');
            }
            out.push('}');
        }
        Value::Object(m) => {
            out.push('{');
            for (k, x) in m {
                out.push('[');
                lua_str(k, out);
                out.push_str("]=");
                write_lua(x, out);
                out.push(',');
            }
            out.push('}');
        }
    }
}

/// Lua helpers, kept in their own file: inside `\directlua{…}` TeX would treat `%` in Lua
/// patterns as a comment and mangle the code.
pub const NERP_LUA: &str = r#"nerp = { data = dofile("data.lua") }
function nerp.get(path)
  local v = nerp.data
  for part in string.gmatch(path, "[^%.]+") do
    if type(v) ~= "table" then return nil end
    local n = tonumber(part)
    if n ~= nil and v[n] ~= nil then v = v[n] else v = v[part] end
  end
  return v
end
function nerp.put(path)
  local v = nerp.get(path)
  if type(v) == "table" then v = v.text end
  if v == nil or v == true or v == false then v = "" end
  -- catcode -2: every character is typeset literally, nothing is interpreted as TeX.
  tex.sprint(-2, tostring(v))
end
function nerp.truthy(path)
  local v = nerp.get(path)
  if type(v) == "table" then
    if v.text ~= nil then return v.text ~= "" end
    return next(v) ~= nil
  end
  return v ~= nil and v ~= "" and v ~= false and v ~= "false"
end
function nerp.rows(path, body)
  local list = nerp.get(path)
  if type(list) ~= "table" then return end
  for i = 1, #list do
    local b = string.gsub(body, "\\crmrow%s*{", "\\crm{" .. path .. "." .. i .. ".")
    tex.print(b)
  end
end
"#;

/// The helper package every LaTeX template can `\usepackage{nerp}`.
pub const NERP_STY: &str = r#"\NeedsTeXFormat{LaTeX2e}
\ProvidesPackage{nerp}[2026/11/07 NERP document data]
\directlua{dofile("nerp.lua")}
% \crm{client.name}  prints a value (money prints its formatted text)
\newcommand\crm[1]{\directlua{nerp.put("\luaescapestring{#1}")}}
% \crmrows{products}{\crmrow{name} & \crmrow{total}\\} repeats the body for each row
\newcommand\crmrows[2]{\directlua{nerp.rows("\luaescapestring{#1}", "\luaescapestring{\unexpanded{#2}}")}}
% \crmif{totals.has_tax}{...} keeps the body only when the value is present/true
\newcommand\crmif[2]{\directlua{if nerp.truthy("\luaescapestring{#1}") then tex.print("\luaescapestring{\unexpanded{#2}}") end}}
% \crmifnot{has_products}{...} keeps the body only when the value is empty/false
\newcommand\crmifnot[2]{\directlua{if not nerp.truthy("\luaescapestring{#1}") then tex.print("\luaescapestring{\unexpanded{#2}}") end}}
\endinput
"#;

/// Renders a LuaLaTeX template. Returns the PDF or the end of the TeX log.
pub async fn render(bin: &str, template: &str, data: &Value, logo: Option<(String, Vec<u8>)>, timeout_secs: u64) -> Result<Vec<u8>, String> {
    let dir = tempfile_dir()?;
    let w = |name: &str, bytes: &[u8]| std::fs::write(dir.join(name), bytes).map_err(|e| e.to_string());
    w("main.tex", template.as_bytes())?;
    w("nerp.sty", NERP_STY.as_bytes())?;
    w("nerp.lua", NERP_LUA.as_bytes())?;
    w("data.lua", format!("return {}\n", to_lua(data)).as_bytes())?;
    if let Some((name, bytes)) = &logo {
        w(name, bytes)?;
    }
    let result = async {
        // Two passes so "page X of Y" and references settle.
        for pass in 0..2 {
            let out = run(
                bin,
                &["--no-shell-escape", "--interaction=nonstopmode", "--halt-on-error", "--file-line-error", "main.tex"],
                &dir,
                &[("openin_any", "p"), ("openout_any", "p"), ("shell_escape", "f"), ("TEXMFOUTPUT", dir.to_str().unwrap_or("."))],
                timeout_secs,
            )
            .await;
            match out {
                Ok(_) => {}
                Err(RunError::Failed(_)) | Err(RunError::Timeout) => {
                    let log = std::fs::read_to_string(dir.join("main.log")).unwrap_or_default();
                    let mut msg = error_lines(&log);
                    if matches!(out, Err(RunError::Timeout)) {
                        msg = format!("LaTeX took too long and was stopped.\n{msg}");
                    }
                    return Err(msg);
                }
                Err(RunError::Spawn(e)) => return Err(format!("could not start lualatex: {e}")),
            }
            let log = std::fs::read_to_string(dir.join("main.log")).unwrap_or_default();
            if pass == 0 && !log.contains("Rerun") && !log.contains("rerun") {
                break;
            }
        }
        std::fs::read(dir.join("main.pdf")).map_err(|_| "LaTeX produced no PDF".to_string())
    }
    .await;
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// The useful part of a TeX log: each error line ("! …" or "file:line: …") plus the
/// few lines of context TeX prints after it. Falls back to the log's tail.
fn error_lines(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    let mut out: Vec<String> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let is_err = l.starts_with("! ") || l.starts_with("[\\directlua]") || l.starts_with("main.tex:") || l.contains(".tex:") && l.contains(": ");
        if is_err && out.len() < 30 {
            out.extend(lines[i..(i + 4).min(lines.len())].iter().filter(|x| !x.trim().is_empty()).map(|x| x.to_string()));
        }
    }
    if out.is_empty() {
        let tail: Vec<&str> = lines.iter().copied().filter(|l| !l.trim().is_empty()).collect();
        return tail[tail.len().saturating_sub(25)..].join("\n");
    }
    out.dedup();
    out.join("\n")
}

fn tempfile_dir() -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("nerp-tex-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lua_literal_escapes_everything() {
        let v = json!({ "a\"b": "x\\y\n\"q\" ]] \u{1}", "n": [1, true, null] });
        let lua = to_lua(&v);
        assert_eq!(lua, r#"{["a\"b"]="x\\y\n\"q\" ]] \001",["n"]={"1",true,"",},}"#);
    }
}
