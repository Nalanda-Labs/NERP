//! Database migrations.
//!
//! Each migration file runs in one transaction (PostgreSQL rolls schema changes back too), so
//! a migration either applies completely or not at all. Its statements are run one by one so
//! an error names the statement that failed. An advisory lock lets several instances start at
//! once: one migrates, the others wait. Applied migrations are recorded in sqlx's
//! `_sqlx_migrations` table, with sqlx's checksums.

use anyhow::{bail, Context, Result};
use sqlx::postgres::PgConnectOptions;
use sqlx::{Acquire, ConnectOptions, PgPool};
use std::str::FromStr;
use std::time::Instant;

/// Splits SQL into statements on `;`, ignoring semicolons inside quotes, comments and
/// dollar-quoted bodies. Comment-only fragments are dropped.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    let (mut single, mut line_comment, mut block_comment) = (false, false, false);
    let mut dollar: Option<String> = None;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if line_comment {
            if c == '\n' {
                line_comment = false;
            }
        } else if block_comment {
            if c == '*' && next == Some('/') {
                block_comment = false;
                cur.push(c);
                cur.push('/');
                i += 2;
                continue;
            }
        } else if let Some(tag) = &dollar {
            if c == '$' && sql[sql.char_indices().nth(i).unwrap().0..].starts_with(tag.as_str()) {
                cur.push_str(tag);
                i += tag.chars().count();
                dollar = None;
                continue;
            }
        } else if single {
            if c == '\'' {
                single = false;
            }
        } else if c == '\'' {
            single = true;
        } else if c == '-' && next == Some('-') {
            line_comment = true;
        } else if c == '/' && next == Some('*') {
            block_comment = true;
        } else if c == '$' {
            // $tag$ … $tag$
            let rest: String = chars[i + 1..].iter().take_while(|ch| ch.is_alphanumeric() || **ch == '_').collect();
            if chars.get(i + 1 + rest.chars().count()) == Some(&'$') {
                let tag = format!("${rest}$");
                cur.push_str(&tag);
                i += tag.chars().count();
                dollar = Some(tag);
                continue;
            }
        } else if c == ';' {
            push(&mut out, &cur);
            cur.clear();
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    push(&mut out, &cur);
    out
}

fn push(out: &mut Vec<String>, stmt: &str) {
    let meaningful = stmt.lines().any(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with("--")
    });
    if meaningful {
        out.push(stmt.trim().to_string());
    }
}

/// Creates the database named in `url` if it doesn't exist yet.
pub async fn ensure_database(url: &str) -> Result<()> {
    let opts = PgConnectOptions::from_str(url).context("parsing DATABASE_URL")?;
    let Some(name) = opts.get_database().map(str::to_owned) else {
        return Ok(());
    };
    // Connect to the default "postgres" database to check for / create ours.
    let mut conn = opts
        .clone()
        .database("postgres")
        .connect()
        .await
        .context("connecting to PostgreSQL")?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
        .bind(&name)
        .fetch_one(&mut conn)
        .await?;
    if !exists {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            anyhow::bail!("database name must be alphanumeric/underscore: {name}");
        }
        // `name` was validated as alphanumeric/underscore above.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}"))).execute(&mut conn).await?;
        tracing::info!("created database {name}");
    }
    sqlx::Connection::close(conn).await?;
    Ok(())
}

/// Any fixed number: the advisory lock that lets only one instance migrate at a time.
const LOCK_KEY: i64 = 0x6e65_7270_6d69_67; // "nerpmig"

pub async fn run(db: &PgPool) -> Result<()> {
    let migrator = sqlx::migrate!("./migrations");
    let mut conn = db.acquire().await?;
    // Instances starting together wait here; the first one migrates, the rest find nothing to do.
    sqlx::query("SELECT pg_advisory_lock($1)").bind(LOCK_KEY).execute(&mut *conn).await?;
    let result = apply(&mut conn, &migrator).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)").bind(LOCK_KEY).execute(&mut *conn).await;
    result
}

async fn apply(conn: &mut sqlx::PgConnection, migrator: &sqlx::migrate::Migrator) -> Result<()> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
             version BIGINT PRIMARY KEY, description TEXT NOT NULL, installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
             success BOOLEAN NOT NULL, checksum BYTEA NOT NULL, execution_time BIGINT NOT NULL)",
    )
    .execute(&mut *conn)
    .await?;
    let applied: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as("SELECT version, success, checksum FROM _sqlx_migrations").fetch_all(&mut *conn).await?;
    for m in migrator.iter() {
        if let Some((_, success, checksum)) = applied.iter().find(|a| a.0 == m.version) {
            if !success {
                bail!("migration {} is marked as failed; fix the database by hand and delete its row from _sqlx_migrations", m.version);
            }
            if checksum.as_slice() != m.checksum.as_ref() {
                bail!("migration {} ({}) was changed after it was applied", m.version, m.description);
            }
            continue;
        }
        let statements = split_statements(m.sql.as_str());
        let started = Instant::now();
        let mut tx = conn.begin().await?;
        for (i, stmt) in statements.iter().enumerate() {
            sqlx::raw_sql(sqlx::AssertSqlSafe(stmt.clone()))
                .execute(&mut *tx)
                .await
                .with_context(|| format!("migration {} ({}), statement {}:\n{}", m.version, m.description, i + 1, first_line(stmt)))?;
        }
        sqlx::query("INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES ($1, $2, TRUE, $3, $4)")
            .bind(m.version)
            .bind(m.description.as_ref())
            .bind(m.checksum.as_ref())
            .bind(started.elapsed().as_nanos() as i64)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(version = m.version, description = %m.description, statements = statements.len(), "applied migration");
    }
    Ok(())
}

fn first_line(s: &str) -> &str {
    s.lines().find(|l| !l.trim().is_empty() && !l.trim().starts_with("--")).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::split_statements;

    #[test]
    fn splits_respecting_quotes_comments_and_dollar_bodies() {
        let sql = "-- header; not a statement\nCREATE TABLE a (x TEXT DEFAULT 'a;b');\n/* block; */ INSERT INTO a VALUES ('it''s;');\n\
                   CREATE FUNCTION f() RETURNS int AS $body$ SELECT 1; $body$ LANGUAGE sql;\n-- trailing comment only\n";
        let s = split_statements(sql);
        assert_eq!(s.len(), 3, "{s:#?}");
        assert!(s[0].ends_with("DEFAULT 'a;b')"));
        assert!(s[1].contains("'it''s;'"));
        assert!(s[2].contains("$body$ SELECT 1; $body$"));
    }
}
