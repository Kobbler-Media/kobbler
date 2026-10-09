use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use password_hash::SaltString;
pub fn hash_password(password: &str) -> Result<String, password_hash::Error> {
    let salt = SaltString::generate(&mut password_hash::rand_core::OsRng);
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    let parsed = PasswordHash::new(hash);
    if parsed.is_err() {
        return false;
    }
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed.unwrap())
        .is_ok()
}
