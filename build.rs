// `sqlx::migrate!()` copies the migration files into the binary at compile time, but
// Cargo doesn't know that: without this, adding a migration wouldn't trigger a rebuild,
// and the server would start with the old set.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
