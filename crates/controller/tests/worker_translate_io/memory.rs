pub(crate) fn sensitive_key(key: &str) -> bool {
    let key = key.replace('\0', "").to_ascii_lowercase().replace('-', "_");
    matches!(
        key.as_str(),
        "key"
            | "key_file"
            | "api_key"
            | "apikey"
            | "password"
            | "passwd"
            | "secret"
            | "token"
            | "access_token"
            | "refresh_token"
            | "authorization"
            | "cookie"
            | "private_key"
            | "credentials"
            | "client_secret"
    ) || key.ends_with("password")
        || key.ends_with("_key")
        || key.ends_with("_secret")
        || key.ends_with("_token")
}
