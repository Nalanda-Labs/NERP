//! Requisites by country: the legal identifiers and bank account fields each country uses,
//! with format checks and checksums where the scheme has one.
//!
//! Values are checked on their "compact" form: upper case, with spaces, dots, dashes and
//! slashes removed. The value is stored as typed; the compact form is indexed for
//! duplicate detection and search.

use serde::Serialize;

#[derive(Clone, Copy)]
pub enum Check {
    Any,
    /// Mini patterns on the compact value: `9` digit, `A` letter, `X` letter or digit,
    /// other characters literal; alternatives separated by `|`.
    Pattern(&'static str),
    Iban,
    Aba,
    Gstin,
    Luhn(&'static str),
    Abn,
    Cnpj,
    Inn,
    Nip,
    /// EU VAT: country prefix (e.g. "DE") then 2–13 letters/digits.
    EuVat(&'static str),
    Email,
}

#[derive(Clone, Copy)]
pub struct Field {
    pub code: &'static str,
    pub label: &'static str,
    pub check: Check,
    /// Shown as an example in the form.
    pub hint: &'static str,
    /// Offered for individuals (sole traders, private persons) as well as organisations.
    pub person: bool,
    /// Printed on documents in the client's tax line.
    pub print: bool,
}

const fn f(code: &'static str, label: &'static str, check: Check, hint: &'static str) -> Field {
    Field { code, label, check, hint, person: false, print: true }
}
/// Also for individuals.
const fn fp(code: &'static str, label: &'static str, check: Check, hint: &'static str) -> Field {
    Field { code, label, check, hint, person: true, print: true }
}
/// Recorded but not printed (e.g. a register court).
const fn fx(code: &'static str, label: &'static str, check: Check, hint: &'static str) -> Field {
    Field { code, label, check, hint, person: false, print: false }
}

use Check::*;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

const GENERIC: &[Field] = &[fp("tax_id", "Tax ID", Any, ""), f("reg_no", "Registration number", Any, "")];

const US: &[Field] = &[fp("ein", "EIN", Pattern("999999999"), "12-3456789"), fx("state_reg", "State registration no.", Any, "")];
const CA: &[Field] = &[fp("bn", "Business Number (BN)", Pattern("999999999|999999999AA9999"), "123456789 RT0001"), fx("qst", "QST number", Any, "")];
const GB: &[Field] = &[fp("vat", "VAT number", Pattern("GB999999999|GB999999999999|GBGD999|GBHA999"), "GB123456789"), f("crn", "Company number", Pattern("99999999|AA999999"), "01234567")];
const IE: &[Field] = &[fp("vat", "VAT number", EuVat("IE"), "IE1234567T"), f("cro", "CRO number", Pattern("999999|9999999"), "123456")];
const DE: &[Field] = &[
    fp("vat", "USt-IdNr.", Pattern("DE999999999"), "DE123456789"),
    fp("tax_no", "Steuernummer", Any, "12/345/67890"),
    f("hrb", "Handelsregister", Any, "HRB 12345"),
    fx("court", "Registergericht", Any, "Amtsgericht Berlin"),
];
const AT: &[Field] = &[fp("vat", "UID-Nummer", Pattern("ATU99999999"), "ATU12345678"), f("fn", "Firmenbuchnummer", Any, "FN 123456a")];
const CH: &[Field] = &[f("uid", "UID", Pattern("CHE999999999"), "CHE-123.456.789"), fp("vat", "MWST-Nr.", Pattern("CHE999999999MWST|CHE999999999TVA|CHE999999999IVA"), "CHE-123.456.789 MWST")];
const FR: &[Field] = &[
    f("siren", "SIREN", Luhn("999999999"), "123 456 789"),
    f("siret", "SIRET", Luhn("99999999999999"), "123 456 789 00012"),
    fp("vat", "N° TVA", Pattern("FRXX999999999"), "FR12345678901"),
    fx("rcs", "RCS", Any, "RCS Paris B 123 456 789"),
    fx("ape", "Code APE", Pattern("9999A"), "6201Z"),
];
const IT: &[Field] = &[
    fp("piva", "Partita IVA", Pattern("99999999999"), "12345678901"),
    fp("cf", "Codice fiscale", Pattern("AAAAAA99A99A999A|99999999999"), "RSSMRA80A01H501U"),
    fx("rea", "REA", Any, "MI-1234567"),
    fx("sdi", "Codice destinatario (SDI)", Pattern("XXXXXXX|XXXXXX"), "ABC1234"),
    fx("pec", "PEC", Email, "fatture@pec.example.it"),
];
const ES: &[Field] = &[fp("nif", "NIF / CIF", Pattern("X9999999X"), "B12345678"), fp("vat", "NIF-IVA", EuVat("ES"), "ESB12345678")];
const PT: &[Field] = &[fp("nif", "NIF", Pattern("999999999"), "123456789"), fp("vat", "NIF-IVA", Pattern("PT999999999"), "PT123456789")];
const NL: &[Field] = &[f("kvk", "KvK-nummer", Pattern("99999999"), "12345678"), fp("vat", "Btw-id", Pattern("NL999999999B99"), "NL123456789B01"), fx("rsin", "RSIN", Pattern("999999999"), "")];
const BE: &[Field] = &[f("kbo", "Enterprise number", Pattern("9999999999"), "0123.456.789"), fp("vat", "VAT / TVA / BTW", Pattern("BE9999999999"), "BE0123456789")];
const LU: &[Field] = &[f("rcs", "RCS", Any, "B123456"), fp("vat", "TVA", Pattern("LU99999999"), "LU12345678")];
const PL: &[Field] = &[fp("nip", "NIP", Nip, "123-456-32-18"), f("regon", "REGON", Pattern("999999999|99999999999999"), "123456789"), fx("krs", "KRS", Pattern("9999999999"), "0000123456"), fp("vat", "VAT UE", EuVat("PL"), "PL1234563218")];
const CZ: &[Field] = &[f("ico", "IČO", Pattern("99999999"), "12345678"), fp("dic", "DIČ", Pattern("CZ99999999|CZ999999999|CZ9999999999"), "CZ12345678")];
const SK: &[Field] = &[f("ico", "IČO", Pattern("99999999"), "12345678"), fp("dic", "DIČ", Pattern("9999999999"), ""), fp("vat", "IČ DPH", Pattern("SK9999999999"), "SK1234567890")];
const SE: &[Field] = &[f("org_no", "Organisationsnummer", Pattern("9999999999"), "556123-4567"), fp("vat", "Momsregistreringsnummer", Pattern("SE999999999901"), "SE556123456701")];
const NO: &[Field] = &[f("org_no", "Organisasjonsnummer", Pattern("999999999"), "123 456 789"), fp("vat", "MVA-nummer", Pattern("NO999999999MVA"), "NO123456789MVA")];
const DK: &[Field] = &[f("cvr", "CVR-nummer", Pattern("99999999"), "12345678"), fp("vat", "VAT", Pattern("DK99999999"), "DK12345678")];
const FI: &[Field] = &[f("y_tunnus", "Y-tunnus", Pattern("99999999"), "1234567-8"), fp("vat", "ALV-numero", Pattern("FI99999999"), "FI12345678")];
const RU: &[Field] = &[fp("inn", "ИНН", Inn, "7707083893"), f("kpp", "КПП", Pattern("9999XX999"), "773601001"), f("ogrn", "ОГРН / ОГРНИП", Pattern("9999999999999|999999999999999"), "1027700132195")];
const UA: &[Field] = &[f("edrpou", "Код ЄДРПОУ", Pattern("99999999"), "12345678"), fp("ipn", "ІПН", Pattern("9999999999|999999999999"), "")];
const KZ: &[Field] = &[fp("bin", "БИН / ИИН", Pattern("999999999999"), "123456789012")];
const TR: &[Field] = &[fp("vkn", "Vergi Kimlik No", Pattern("9999999999|99999999999"), "1234567890"), fx("tax_office", "Vergi Dairesi", Any, "Kadıköy"), f("mersis", "MERSİS No", Pattern("9999999999999999"), "")];
const IN: &[Field] = &[
    fp("gstin", "GSTIN", Gstin, "27AAPFU0939F1ZV"),
    fp("pan", "PAN", Pattern("AAAAA9999A"), "ABCDE1234F"),
    f("cin", "CIN", Pattern("A99999AA9999AAA999999"), "U72200KA2010PTC012345"),
    fx("tan", "TAN", Pattern("AAAA99999A"), "BLRA12345B"),
];
const CN: &[Field] = &[f("uscc", "统一社会信用代码", Pattern("XXXXXXXXXXXXXXXXXX"), "91110000600037341L")];
const JP: &[Field] = &[f("corp_no", "法人番号", Pattern("9999999999999"), "1234567890123"), fp("invoice_no", "登録番号 (インボイス)", Pattern("T9999999999999"), "T1234567890123")];
const KR: &[Field] = &[fp("brn", "사업자등록번호", Pattern("9999999999"), "123-45-67890")];
const SG: &[Field] = &[f("uen", "UEN", Pattern("99999999A|999999999A|A99AA9999A"), "201912345A"), fp("gst", "GST reg. no.", Any, "M90312345A")];
const MY: &[Field] = &[f("brn", "SSM registration no.", Any, "202001012345"), fp("tin", "TIN", Any, "C1234567890"), f("sst", "SST no.", Any, "")];
const ID: &[Field] = &[fp("npwp", "NPWP", Pattern("999999999999999|9999999999999999"), "01.234.567.8-901.000"), fx("nib", "NIB", Pattern("9999999999999"), "")];
const TH: &[Field] = &[fp("tax_id", "เลขประจำตัวผู้เสียภาษี", Pattern("9999999999999"), "0105551234567")];
const VN: &[Field] = &[fp("mst", "Mã số thuế", Pattern("9999999999|9999999999999"), "0101234567")];
const PH: &[Field] = &[fp("tin", "TIN", Pattern("999999999|999999999999|99999999999999"), "123-456-789-000")];
const AU: &[Field] = &[fp("abn", "ABN", Abn, "51 824 753 556"), f("acn", "ACN", Pattern("999999999"), "004 085 616")];
const NZ: &[Field] = &[f("nzbn", "NZBN", Pattern("9999999999999"), "9429041234567"), fp("gst", "GST number", Pattern("99999999|999999999"), "123-456-789")];
const BR: &[Field] = &[
    f("cnpj", "CNPJ", Cnpj, "11.222.333/0001-81"),
    Field { code: "cpf", label: "CPF", check: Pattern("99999999999"), hint: "123.456.789-09", person: true, print: true },
    f("ie", "Inscrição Estadual", Any, ""),
    fx("im", "Inscrição Municipal", Any, ""),
];
const MX: &[Field] = &[fp("rfc", "RFC", Pattern("AAA999999XXX|AAAA999999XXX"), "ABC010203XY1"), fx("regimen", "Régimen fiscal", Pattern("999"), "601")];
const AR: &[Field] = &[fp("cuit", "CUIT / CUIL", Pattern("99999999999"), "30-12345678-9"), fx("iva", "Condición IVA", Any, "Responsable Inscripto")];
const CL: &[Field] = &[fp("rut", "RUT", Pattern("99999999X|9999999X"), "76.123.456-K"), fx("giro", "Giro", Any, "")];
const CO: &[Field] = &[fp("nit", "NIT", Pattern("9999999999|999999999"), "900.123.456-7")];
const PE: &[Field] = &[fp("ruc", "RUC", Pattern("99999999999"), "20123456789")];
const ZA: &[Field] = &[f("reg_no", "CIPC registration no.", Pattern("999999999999"), "2019/123456/07"), fp("vat", "VAT number", Pattern("4999999999"), "4123456789")];
const NG: &[Field] = &[f("rc", "RC number", Any, "RC 123456"), fp("tin", "TIN", Pattern("99999999|999999999999"), "12345678-0001")];
const KE: &[Field] = &[fp("pin", "KRA PIN", Pattern("A999999999A"), "P051234567A"), f("reg_no", "Registration no.", Any, "")];
const EG: &[Field] = &[fp("tax_reg", "Tax registration no.", Pattern("999999999"), "123-456-789"), f("cr", "Commercial register", Any, "")];
const AE: &[Field] = &[fp("trn", "TRN", Pattern("999999999999999"), "100123456700003"), f("license", "Trade licence no.", Any, "")];
const SA: &[Field] = &[fp("vat", "VAT number", Pattern("399999999999993"), "310123456700003"), f("cr", "CR number", Pattern("9999999999"), "1010123456")];
const IL: &[Field] = &[fp("company_no", "Company / dealer no.", Pattern("999999999"), "512345678")];

/// Other EU members: VAT plus a national registration number.
const EU_OTHER: &[(&str, &str)] = &[
    ("GR", "EL"), ("HU", "HU"), ("RO", "RO"), ("BG", "BG"), ("HR", "HR"), ("SI", "SI"),
    ("LT", "LT"), ("LV", "LV"), ("EE", "EE"), ("CY", "CY"), ("MT", "MT"),
];

macro_rules! eu_other {
    ($($c:literal => $p:literal),*) => {
        fn eu_other(country: &str) -> Option<&'static [Field]> {
            match country {
                $($c => {
                    const F: &[Field] = &[fp("vat", "VAT number", EuVat($p), concat!($p, "12345678")), f("reg_no", "Registration number", Any, "")];
                    Some(F)
                })*
                _ => None,
            }
        }
    };
}
eu_other!("GR" => "EL", "HU" => "HU", "RO" => "RO", "BG" => "BG", "HR" => "HR", "SI" => "SI", "LT" => "LT", "LV" => "LV", "EE" => "EE", "CY" => "CY", "MT" => "MT");

pub fn identifiers(country: &str) -> &'static [Field] {
    match country {
        "US" => US, "CA" => CA, "GB" => GB, "IE" => IE, "DE" => DE, "AT" => AT, "CH" | "LI" => CH,
        "FR" => FR, "IT" => IT, "ES" => ES, "PT" => PT, "NL" => NL, "BE" => BE, "LU" => LU,
        "PL" => PL, "CZ" => CZ, "SK" => SK, "SE" => SE, "NO" => NO, "DK" => DK, "FI" => FI,
        "RU" => RU, "UA" => UA, "KZ" => KZ, "TR" => TR, "IN" => IN, "CN" => CN, "JP" => JP,
        "KR" => KR, "SG" => SG, "MY" => MY, "ID" => ID, "TH" => TH, "VN" => VN, "PH" => PH,
        "AU" => AU, "NZ" => NZ, "BR" => BR, "MX" => MX, "AR" => AR, "CL" => CL, "CO" => CO,
        "PE" => PE, "ZA" => ZA, "NG" => NG, "KE" => KE, "EG" => EG, "AE" => AE, "SA" => SA,
        "IL" => IL,
        other => eu_other(other).unwrap_or(GENERIC),
    }
}

/// Countries with a specific set (everything else gets the generic one).
pub fn specific_countries() -> Vec<&'static str> {
    let mut v = vec![
        "US", "CA", "GB", "IE", "DE", "AT", "CH", "LI", "FR", "IT", "ES", "PT", "NL", "BE", "LU", "PL", "CZ", "SK", "SE",
        "NO", "DK", "FI", "RU", "UA", "KZ", "TR", "IN", "CN", "JP", "KR", "SG", "MY", "ID", "TH", "VN", "PH", "AU", "NZ",
        "BR", "MX", "AR", "CL", "CO", "PE", "ZA", "NG", "KE", "EG", "AE", "SA", "IL",
    ];
    v.extend(EU_OTHER.iter().map(|(c, _)| *c));
    v
}

// ---------------------------------------------------------------------------
// Bank accounts
// ---------------------------------------------------------------------------

const fn b(code: &'static str, label: &'static str, check: Check, hint: &'static str) -> Field {
    Field { code, label, check, hint, person: true, print: true }
}

const BANK_NAME: Field = b("bank_name", "Bank", Any, "");
const SWIFT: Field = b("bic", "BIC / SWIFT", Pattern("AAAAAAXX|AAAAAAXXXXX"), "DEUTDEFF");

const BANK_IBAN: &[Field] = &[BANK_NAME, b("iban", "IBAN", Iban, "DE89 3704 0044 0532 0130 00"), SWIFT];
const BANK_US: &[Field] = &[BANK_NAME, b("routing", "Routing number (ABA)", Aba, "021000021"), b("account", "Account number", Any, ""), SWIFT];
const BANK_CA: &[Field] = &[BANK_NAME, b("institution", "Institution no.", Pattern("999"), "003"), b("transit", "Transit no.", Pattern("99999"), "12345"), b("account", "Account number", Any, ""), SWIFT];
const BANK_GB: &[Field] = &[BANK_NAME, b("sort_code", "Sort code", Pattern("999999"), "12-34-56"), b("account", "Account number", Pattern("99999999"), "12345678"), b("iban", "IBAN", Iban, ""), SWIFT];
const BANK_IN: &[Field] = &[BANK_NAME, b("ifsc", "IFSC", Pattern("AAAA0XXXXXX"), "HDFC0000123"), b("account", "Account number", Any, ""), SWIFT];
const BANK_AU: &[Field] = &[BANK_NAME, b("bsb", "BSB", Pattern("999999"), "062-000"), b("account", "Account number", Any, ""), SWIFT];
const BANK_NZ: &[Field] = &[BANK_NAME, b("account", "Account number", Pattern("999999999999999|9999999999999999"), "12-3456-7890123-00"), SWIFT];
const BANK_BR: &[Field] = &[BANK_NAME, b("bank_code", "Código do banco", Pattern("999"), "001"), b("agency", "Agência", Any, "1234-5"), b("account", "Conta", Any, ""), b("pix", "Chave Pix", Any, "")];
const BANK_MX: &[Field] = &[BANK_NAME, b("clabe", "CLABE", Pattern("999999999999999999"), "002010077777777771"), SWIFT];
const BANK_AR: &[Field] = &[BANK_NAME, b("cbu", "CBU / CVU", Pattern("9999999999999999999999"), ""), b("alias", "Alias", Any, "")];
const BANK_RU: &[Field] = &[BANK_NAME, b("bik", "БИК", Pattern("999999999"), "044525225"), b("account", "Расчётный счёт", Pattern("99999999999999999999"), ""), b("corr_account", "Корр. счёт", Pattern("99999999999999999999"), "")];
const BANK_JP: &[Field] = &[BANK_NAME, b("branch", "Branch (支店)", Any, ""), b("account_type", "Account type (普通/当座)", Any, "普通"), b("account", "Account number", Pattern("9999999"), ""), SWIFT];
const BANK_CN: &[Field] = &[BANK_NAME, b("account", "Account number", Any, ""), b("cnaps", "CNAPS code", Pattern("999999999999"), ""), SWIFT];
const BANK_GENERIC: &[Field] = &[BANK_NAME, b("account", "Account number", Any, ""), b("iban", "IBAN", Iban, ""), SWIFT];

/// IBAN countries (as used for domestic and SEPA payments).
const IBAN_COUNTRIES: &[&str] = &[
    "AT", "BE", "BG", "CH", "CY", "CZ", "DE", "DK", "EE", "ES", "FI", "FR", "GR", "HR", "HU", "IE", "IS", "IT", "LI",
    "LT", "LU", "LV", "MC", "MT", "NL", "NO", "PL", "PT", "RO", "SE", "SI", "SK", "SM", "AD", "GI", "AL", "BA", "ME",
    "MK", "RS", "MD", "UA", "GE", "AZ", "KZ", "TR", "IL", "JO", "LB", "KW", "BH", "QA", "SA", "AE", "EG", "TN", "PK",
    "MR", "BY", "DO", "CR", "GT", "SV", "VG", "XK", "LC", "SC", "IQ", "LY", "SD",
];

pub fn bank(country: &str) -> &'static [Field] {
    match country {
        "US" => BANK_US, "CA" => BANK_CA, "GB" => BANK_GB, "IN" => BANK_IN, "AU" => BANK_AU, "NZ" => BANK_NZ,
        "BR" => BANK_BR, "MX" => BANK_MX, "AR" => BANK_AR, "RU" => BANK_RU, "JP" => BANK_JP, "CN" => BANK_CN,
        c if IBAN_COUNTRIES.contains(&c) => BANK_IBAN,
        _ => BANK_GENERIC,
    }
}

// ---------------------------------------------------------------------------
// Checking values
// ---------------------------------------------------------------------------

/// Upper case, letters and digits only.
pub fn compact(v: &str) -> String {
    v.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_uppercase).collect()
}

fn pattern_ok(p: &str, v: &str) -> bool {
    p.split('|').any(|alt| {
        alt.chars().count() == v.chars().count()
            && alt.chars().zip(v.chars()).all(|(pc, vc)| match pc {
                '9' => vc.is_ascii_digit(),
                'A' => vc.is_ascii_uppercase(),
                'X' => vc.is_ascii_uppercase() || vc.is_ascii_digit(),
                lit => lit == vc,
            })
    })
}

fn digits(v: &str) -> Option<Vec<u32>> {
    v.chars().map(|c| c.to_digit(10)).collect()
}

pub fn iban_ok(v: &str) -> bool {
    if v.len() < 15 || v.len() > 34 || !v.chars().all(|c| c.is_ascii_alphanumeric()) || !v[..2].chars().all(|c| c.is_ascii_uppercase()) || !v[2..4].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let rearranged = format!("{}{}", &v[4..], &v[..4]);
    let mut rem: u64 = 0;
    for c in rearranged.chars() {
        let n = c.to_digit(36).unwrap() as u64;
        rem = if n >= 10 { (rem * 100 + n) % 97 } else { (rem * 10 + n) % 97 };
    }
    rem == 1
}

fn aba_ok(v: &str) -> bool {
    let Some(d) = digits(v).filter(|d| d.len() == 9) else { return false };
    let w = [3, 7, 1, 3, 7, 1, 3, 7, 1];
    d.iter().zip(w).map(|(a, b)| a * b).sum::<u32>() % 10 == 0
}

fn luhn_ok(v: &str) -> bool {
    let Some(d) = digits(v) else { return false };
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| if i % 2 == 1 { let y = x * 2; if y > 9 { y - 9 } else { y } } else { x })
        .sum();
    sum.is_multiple_of(10)
}

fn gstin_ok(v: &str) -> bool {
    if !pattern_ok("99AAAAA9999AXZX", v) {
        return false;
    }
    const CHARS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut sum = 0u32;
    for (i, c) in v[..14].chars().enumerate() {
        let p = CHARS.find(c).unwrap() as u32 * if i % 2 == 1 { 2 } else { 1 };
        sum += p / 36 + p % 36;
    }
    let check = (36 - sum % 36) % 36;
    CHARS.as_bytes()[check as usize] as char == v.chars().nth(14).unwrap()
}

fn abn_ok(v: &str) -> bool {
    let Some(mut d) = digits(v).filter(|d| d.len() == 11) else { return false };
    if d[0] == 0 {
        return false;
    }
    d[0] -= 1;
    let w = [10, 1, 3, 5, 7, 9, 11, 13, 15, 17, 19];
    d.iter().zip(w).map(|(a, b)| a * b).sum::<u32>() % 89 == 0
}

fn cnpj_ok(v: &str) -> bool {
    let Some(d) = digits(v).filter(|d| d.len() == 14) else { return false };
    if d.iter().all(|&x| x == d[0]) {
        return false;
    }
    let calc = |n: usize| {
        let w: Vec<u32> = if n == 12 { vec![5, 4, 3, 2, 9, 8, 7, 6, 5, 4, 3, 2] } else { vec![6, 5, 4, 3, 2, 9, 8, 7, 6, 5, 4, 3, 2] };
        let s: u32 = d[..n].iter().zip(&w).map(|(a, b)| a * b).sum();
        if s % 11 < 2 { 0 } else { 11 - s % 11 }
    };
    calc(12) == d[12] && calc(13) == d[13]
}

fn inn_ok(v: &str) -> bool {
    let Some(d) = digits(v) else { return false };
    let check = |w: &[u32], n: usize| d[..w.len()].iter().zip(w).map(|(a, b)| a * b).sum::<u32>() % 11 % 10 == d[n];
    match d.len() {
        10 => check(&[2, 4, 10, 3, 5, 9, 4, 6, 8], 9),
        12 => check(&[7, 2, 4, 10, 3, 5, 9, 4, 6, 8], 10) && check(&[3, 7, 2, 4, 10, 3, 5, 9, 4, 6, 8], 11),
        _ => false,
    }
}

fn nip_ok(v: &str) -> bool {
    let Some(d) = digits(v).filter(|d| d.len() == 10) else { return false };
    let w = [6, 5, 7, 2, 3, 4, 5, 6, 7];
    let s = d[..9].iter().zip(w).map(|(a, b)| a * b).sum::<u32>() % 11;
    s != 10 && s == d[9]
}

/// Checks one value. Returns an error message for the user, naming the field.
pub fn check(field: &Field, raw: &str) -> Result<(), String> {
    let v = compact(raw);
    let ok = match field.check {
        Any => true,
        Email => raw.contains('@') && raw.contains('.') && !raw.contains(char::is_whitespace),
        Pattern(p) => pattern_ok(p, &v),
        Iban => iban_ok(&v),
        Aba => aba_ok(&v),
        Gstin => gstin_ok(&v),
        Luhn(p) => pattern_ok(p, &v) && luhn_ok(&v),
        Abn => abn_ok(&v),
        Cnpj => cnpj_ok(&v),
        Inn => inn_ok(&v),
        Nip => nip_ok(&v),
        EuVat(prefix) => v.starts_with(prefix) && (2..=13).contains(&(v.len() - prefix.len())),
    };
    if ok {
        Ok(())
    } else if field.hint.is_empty() {
        Err(format!("{} is not valid", field.label))
    } else {
        Err(format!("{} is not valid (example: {})", field.label, field.hint))
    }
}

// ---------------------------------------------------------------------------
// Description for the UI
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct FieldInfo {
    code: &'static str,
    label: &'static str,
    hint: &'static str,
    person: bool,
}

fn info(fields: &[Field]) -> Vec<FieldInfo> {
    fields.iter().map(|f| FieldInfo { code: f.code, label: f.label, hint: f.hint, person: f.person }).collect()
}

pub fn describe(country: &str) -> serde_json::Value {
    serde_json::json!({ "country": country, "fields": info(identifiers(country)), "bank": info(bank(country)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(country: &str, code: &str, v: &str) -> bool {
        let f = identifiers(country).iter().chain(bank(country)).find(|f| f.code == code).unwrap();
        check(f, v).is_ok()
    }

    #[test]
    fn checksums() {
        assert!(ok("DE", "iban", "DE89 3704 0044 0532 0130 00"));
        assert!(!ok("DE", "iban", "DE89 3704 0044 0532 0130 01"));
        assert!(ok("GB", "iban", "GB82 WEST 1234 5698 7654 32"));
        assert!(ok("US", "routing", "021000021"));
        assert!(!ok("US", "routing", "021000022"));
        assert!(ok("IN", "gstin", "27AAPFU0939F1ZV"));
        assert!(!ok("IN", "gstin", "27AAPFU0939F1ZX"));
        assert!(ok("IN", "pan", "abcde1234f"));
        assert!(ok("AU", "abn", "51 824 753 556"));
        assert!(!ok("AU", "abn", "51 824 753 557"));
        assert!(ok("BR", "cnpj", "11.222.333/0001-81"));
        assert!(!ok("BR", "cnpj", "11.222.333/0001-82"));
        assert!(ok("RU", "inn", "7707083893"));
        assert!(ok("RU", "inn", "500100732259"));
        assert!(!ok("RU", "inn", "7707083894"));
        assert!(ok("PL", "nip", "123-456-32-18"));
        assert!(!ok("PL", "nip", "123-456-32-19"));
        assert!(ok("FR", "siren", "732 829 320"));
        assert!(!ok("FR", "siren", "732 829 321"));
    }

    #[test]
    fn patterns_and_fallbacks() {
        assert!(ok("DE", "vat", "DE 123 456 789"));
        assert!(!ok("DE", "vat", "DE12345678"));
        assert!(ok("GR", "vat", "EL123456789"));
        assert!(ok("CH", "uid", "CHE-123.456.789"));
        assert!(ok("NL", "vat", "NL123456789B01"));
        assert!(ok("GB", "sort_code", "12-34-56"));
        assert!(ok("IN", "ifsc", "HDFC0000123"));
        assert!(ok("ZZ", "tax_id", "anything at all"));
        assert_eq!(identifiers("ZZ").len(), 2);
        assert_eq!(bank("FR")[1].code, "iban");
        let e = check(&identifiers("IN")[0], "29ABCDE").unwrap_err();
        assert!(e.contains("GSTIN") && e.contains("example"), "{e}");
    }
}
