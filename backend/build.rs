// Migrations are embedded in the binary (sqlx::migrate!), so a new or changed migration
// file must rebuild it.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
