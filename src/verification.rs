use rand::Rng;

/// Generates a 6-digit verification code as a String
/// Each digit is randomly generated between 0-9
pub fn generate_verification_code() -> String {
    let mut rng = rand::rng();
    let code: String = (0..6)
        .map(|_| rng.random_range(0..10).to_string())
        .collect();
    code
}
