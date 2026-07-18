// @anchor auth.issue-token target=rust://auth::issue_token
// @strand session-token-format role=producer
pub fn issue_token(subject: &str) -> String {
    format!("subject={subject};expires=3600")
}

// @anchor auth.verify-token target=rust://auth::verify_token
// @strand session-token-format role=consumer
pub fn verify_token(token: &str) -> bool {
    token.contains("subject=") && token.contains("expires=")
}
