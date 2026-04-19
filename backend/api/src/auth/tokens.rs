use std::time::Duration;

use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use listenai_core::domain::UserRole;
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use uuid::Uuid;

use super::claims::AccessClaims;

/// Sign an access-token JWT for `(user, role)`, valid for `ttl`.
pub fn issue_access_token(
    user_id: &UserId,
    role: UserRole,
    secret: &str,
    ttl: Duration,
) -> Result<String> {
    let now = Utc::now().timestamp();
    let exp = now + ttl.as_secs() as i64;
    let claims = AccessClaims {
        sub: user_id.clone(),
        role,
        iat: now,
        exp,
        jti: Uuid::new_v4().simple().to_string(),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| Error::Other(anyhow::anyhow!("sign jwt: {e}")))
}

/// Validate and decode an access-token JWT. Returns `Error::Unauthorized` on
/// any failure, so callers can turn it into a 401 without leaking details.
pub fn verify_access_token(token: &str, secret: &str) -> Result<AccessClaims> {
    decode::<AccessClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|_| Error::Unauthorized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn access_token_round_trip() {
        let uid = UserId::new();
        let secret = "test-secret";
        let token = issue_access_token(&uid, UserRole::Admin, secret, Duration::from_secs(60))
            .expect("issue");
        let claims = verify_access_token(&token, secret).expect("verify");
        assert_eq!(claims.sub, uid);
        assert_eq!(claims.role, UserRole::Admin);
    }

    #[test]
    fn wrong_secret_rejects() {
        let uid = UserId::new();
        let token = issue_access_token(&uid, UserRole::User, "secret-a", Duration::from_secs(60))
            .expect("issue");
        assert!(matches!(
            verify_access_token(&token, "secret-b"),
            Err(Error::Unauthorized)
        ));
    }

    #[test]
    fn expired_token_rejects() {
        let uid = UserId::new();
        // Negative TTL → exp is in the past. JWT's default validation clock
        // tolerance is 60 seconds, so we go well past that.
        let token =
            issue_access_token(&uid, UserRole::User, "sec", Duration::from_secs(0)).expect("issue");
        // Sleep one second to clear any tolerance window.
        std::thread::sleep(Duration::from_secs(2));
        // jsonwebtoken's default leeway is 60s, so wait briefly then accept
        // the test may still pass — instead manually build an expired token.
        let expired_claims = AccessClaims {
            sub: uid,
            role: UserRole::User,
            iat: 0,
            exp: 1,
            jti: "x".into(),
        };
        let expired = encode(
            &Header::default(),
            &expired_claims,
            &EncodingKey::from_secret(b"sec"),
        )
        .unwrap();
        assert!(matches!(
            verify_access_token(&expired, "sec"),
            Err(Error::Unauthorized)
        ));
        // silence unused warning
        let _ = token;
    }
}
