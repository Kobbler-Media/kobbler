use argon2::Argon2;
use argon2::password_hash::{self, PasswordHasher, PasswordVerifier};

pub fn hash_password(password: &str) -> Result<String, password_hash::Error> {
    Ok(Argon2::default()
        .hash_password(password.as_bytes())?
        .to_string())
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}
