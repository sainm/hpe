use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, VerifyingKey};
use ring::signature::{ECDSA_P256_SHA256_FIXED, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
use serde_json::Value;

use crate::enterprise_ops::{OidcPrincipal, OpsError};

const CLOCK_SKEW_SECONDS: i64 = 60;
const RSA_MIN_BITS: usize = 2048;
const RSA_MAX_BITS: usize = 8192;

#[derive(Clone, Debug)]
pub(crate) enum JwkKey {
    Ed25519([u8; 32]),
    Rsa { der: Vec<u8>, modulus_len: usize },
    Ec { x: [u8; 32], y: [u8; 32] },
}

#[derive(Clone, Debug)]
pub(crate) struct Jwk {
    pub kid: Option<String>,
    pub key: JwkKey,
}

pub(crate) fn parse_jwks(document: &Value) -> Result<Vec<Jwk>, OpsError> {
    let entries = document
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| OpsError::Upstream("JWKS document has no keys array".to_string()))?;
    let mut keys = Vec::new();
    for entry in entries {
        let kty = entry.get("kty").and_then(Value::as_str).unwrap_or("");
        let key = match kty {
            "OKP" => {
                if entry.get("crv").and_then(Value::as_str) != Some("Ed25519") {
                    continue;
                }
                let x = entry
                    .get("x")
                    .and_then(Value::as_str)
                    .ok_or_else(|| OpsError::Upstream("JWKS Ed25519 key has no x".to_string()))?;
                let decoded = decode_jwk_value(x, "Ed25519 x")?;
                let key: [u8; 32] = decoded.try_into().map_err(|_| {
                    OpsError::Upstream("JWKS Ed25519 x must be 32 bytes".to_string())
                })?;
                JwkKey::Ed25519(key)
            }
            "RSA" => parse_rsa_key(entry)?,
            "EC" => {
                if entry.get("crv").and_then(Value::as_str) != Some("P-256") {
                    continue;
                }
                let x = entry
                    .get("x")
                    .and_then(Value::as_str)
                    .ok_or_else(|| OpsError::Upstream("JWKS EC key has no x".to_string()))?;
                let y = entry
                    .get("y")
                    .and_then(Value::as_str)
                    .ok_or_else(|| OpsError::Upstream("JWKS EC key has no y".to_string()))?;
                let x: [u8; 32] = decode_jwk_value(x, "EC x")?
                    .try_into()
                    .map_err(|_| OpsError::Upstream("JWKS EC x must be 32 bytes".to_string()))?;
                let y: [u8; 32] = decode_jwk_value(y, "EC y")?
                    .try_into()
                    .map_err(|_| OpsError::Upstream("JWKS EC y must be 32 bytes".to_string()))?;
                JwkKey::Ec { x, y }
            }
            _ => continue,
        };
        keys.push(Jwk {
            kid: entry.get("kid").and_then(Value::as_str).map(str::to_string),
            key,
        });
    }
    Ok(keys)
}

fn decode_jwk_value(raw: &str, label: &str) -> Result<Vec<u8>, OpsError> {
    URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| OpsError::Upstream(format!("JWKS {label} is not base64url")))
}

fn parse_rsa_key(entry: &Value) -> Result<JwkKey, OpsError> {
    let n_raw = entry
        .get("n")
        .and_then(Value::as_str)
        .ok_or_else(|| OpsError::Upstream("JWKS RSA key has no n".to_string()))?;
    let e_raw = entry
        .get("e")
        .and_then(Value::as_str)
        .ok_or_else(|| OpsError::Upstream("JWKS RSA key has no e".to_string()))?;
    let modulus = decode_jwk_value(n_raw, "RSA n")?;
    let exponent = decode_jwk_value(e_raw, "RSA e")?;
    if modulus.is_empty() || exponent.is_empty() {
        return Err(OpsError::Upstream(
            "JWKS RSA parameters are empty".to_string(),
        ));
    }
    if modulus[0] == 0 || exponent[0] == 0 {
        return Err(OpsError::Upstream(
            "JWKS RSA parameters have invalid leading bytes".to_string(),
        ));
    }
    let bits = (modulus.len() - 1) * 8 + (8 - modulus[0].leading_zeros() as usize);
    if !(RSA_MIN_BITS..=RSA_MAX_BITS).contains(&bits) {
        return Err(OpsError::Upstream(
            "JWKS RSA modulus must be between 2048 and 8192 bits".to_string(),
        ));
    }
    let der = rsa_pkcs1_der(&modulus, &exponent);
    Ok(JwkKey::Rsa {
        der,
        modulus_len: modulus.len(),
    })
}

fn rsa_pkcs1_der(modulus: &[u8], exponent: &[u8]) -> Vec<u8> {
    let mut body = der_integer(modulus);
    body.extend(der_integer(exponent));
    let mut der = vec![0x30];
    der.extend(der_length(body.len()));
    der.extend(body);
    der
}

fn der_integer(value: &[u8]) -> Vec<u8> {
    let mut content = Vec::with_capacity(value.len() + 1);
    if value[0] & 0x80 != 0 {
        content.push(0);
    }
    content.extend_from_slice(value);
    let mut encoded = vec![0x02];
    encoded.extend(der_length(content.len()));
    encoded.extend(content);
    encoded
}

fn der_length(length: usize) -> Vec<u8> {
    if length < 0x80 {
        vec![length as u8]
    } else if length < 0x100 {
        vec![0x81, length as u8]
    } else {
        vec![0x82, (length >> 8) as u8, length as u8]
    }
}

pub(crate) fn verify_jwt(
    token: &str,
    keys: &[Jwk],
    issuer: &str,
    audience: &str,
    expected_nonce: Option<&str>,
) -> Result<OidcPrincipal, OpsError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return Err(OpsError::Invalid("token is not a compact JWT".to_string()));
    }
    let header_bytes = URL_SAFE_NO_PAD
        .decode(parts[0])
        .map_err(|_| OpsError::Invalid("token header is not base64url".to_string()))?;
    let header: Value = serde_json::from_slice(&header_bytes)?;
    let algorithm = header
        .get("alg")
        .and_then(Value::as_str)
        .ok_or_else(|| OpsError::Invalid("token has no algorithm".to_string()))?;
    if !matches!(algorithm, "EdDSA" | "RS256" | "ES256") {
        return Err(OpsError::Invalid(
            "token algorithm is not supported".to_string(),
        ));
    }
    let kid = match header.get("kid") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| OpsError::Invalid("token kid must be a string".to_string()))?,
        ),
    };
    let key = match kid {
        Some(kid) => keys
            .iter()
            .find(|key| key.kid.as_deref() == Some(kid))
            .ok_or_else(|| OpsError::Invalid("no JWKS key matches the token kid".to_string()))?,
        None if keys.len() == 1 => &keys[0],
        None => {
            return Err(OpsError::Invalid(
                "token has no kid and JWKS has multiple keys".to_string(),
            ));
        }
    };
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| OpsError::Invalid("token signature is not base64url".to_string()))?;
    let signing_input = format!("{}.{}", parts[0], parts[1]);
    verify_signature(
        &key.key,
        algorithm,
        signing_input.as_bytes(),
        &signature_bytes,
    )?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| OpsError::Invalid("token payload is not base64url".to_string()))?;
    let claims: Value = serde_json::from_slice(&payload_bytes)?;
    validate_claims(&claims, issuer, audience, expected_nonce)
}

pub(crate) fn verify_signature(
    key: &JwkKey,
    algorithm: &str,
    message: &[u8],
    signature: &[u8],
) -> Result<(), OpsError> {
    let valid = match (key, algorithm) {
        (JwkKey::Ed25519(bytes), "EdDSA") => {
            if signature.len() != 64 {
                return Err(invalid_signature());
            }
            let verifying_key = VerifyingKey::from_bytes(bytes).map_err(|_| {
                OpsError::Upstream("JWKS contains an invalid Ed25519 key".to_string())
            })?;
            let signature: [u8; 64] = signature.try_into().map_err(|_| invalid_signature())?;
            verifying_key
                .verify_strict(message, &Signature::from_bytes(&signature))
                .is_ok()
        }
        (JwkKey::Rsa { der, modulus_len }, "RS256") => {
            if signature.len() != *modulus_len {
                return Err(invalid_signature());
            }
            UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, der.as_slice())
                .verify(message, signature)
                .is_ok()
        }
        (JwkKey::Ec { x, y }, "ES256") => {
            if signature.len() != 64 {
                return Err(invalid_signature());
            }
            let mut public_key = [0u8; 65];
            public_key[0] = 0x04;
            public_key[1..33].copy_from_slice(x);
            public_key[33..].copy_from_slice(y);
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public_key)
                .verify(message, signature)
                .is_ok()
        }
        _ => {
            return Err(OpsError::Invalid(
                "token algorithm does not match the signing key".to_string(),
            ));
        }
    };
    if valid {
        Ok(())
    } else {
        Err(invalid_signature())
    }
}

fn invalid_signature() -> OpsError {
    OpsError::Invalid("token signature is invalid".to_string())
}

fn validate_claims(
    claims: &Value,
    issuer: &str,
    audience: &str,
    expected_nonce: Option<&str>,
) -> Result<OidcPrincipal, OpsError> {
    let object = claims
        .as_object()
        .ok_or_else(|| OpsError::Invalid("token payload must be a JSON object".to_string()))?;
    let now = now_ms() / 1000;
    let exp = object
        .get("exp")
        .and_then(number_i64)
        .ok_or_else(|| OpsError::Invalid("token is missing exp".to_string()))?;
    if now > exp + CLOCK_SKEW_SECONDS {
        return Err(OpsError::Invalid("token has expired".to_string()));
    }
    if let Some(nbf) = object.get("nbf").and_then(number_i64)
        && now + CLOCK_SKEW_SECONDS < nbf
    {
        return Err(OpsError::Invalid("token is not valid yet".to_string()));
    }
    if let Some(expected) = expected_nonce {
        let nonce = object
            .get("nonce")
            .and_then(Value::as_str)
            .ok_or_else(|| OpsError::Invalid("token is missing nonce".to_string()))?;
        if nonce != expected {
            return Err(OpsError::Invalid("token nonce does not match".to_string()));
        }
    }
    let token_issuer = object
        .get("iss")
        .and_then(Value::as_str)
        .ok_or_else(|| OpsError::Invalid("token is missing iss".to_string()))?;
    if token_issuer != issuer {
        return Err(OpsError::Invalid("token issuer does not match".to_string()));
    }
    let audience_ok = match object.get("aud") {
        Some(Value::String(value)) => value == audience,
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(audience)),
        _ => false,
    };
    if !audience_ok {
        return Err(OpsError::Invalid(
            "token audience does not match".to_string(),
        ));
    }
    let subject = object
        .get("sub")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OpsError::Invalid("token is missing sub".to_string()))?
        .to_string();
    let tenant_id = object
        .get("tenant_id")
        .or_else(|| object.get("tenantId"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OpsError::Invalid("token is missing tenant_id".to_string()))?
        .to_string();
    let role =
        claim_role(object).ok_or_else(|| OpsError::Invalid("token is missing role".to_string()))?;
    if !matches!(
        role.as_str(),
        "owner"
            | "admin"
            | "developer"
            | "publisher"
            | "operator"
            | "approver"
            | "auditor"
            | "viewer"
    ) {
        return Err(OpsError::Invalid(format!(
            "token role `{role}` is not recognized"
        )));
    }
    let email = object
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_string);
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Ok(OidcPrincipal {
        subject,
        tenant_id,
        role,
        email,
        name,
    })
}

fn claim_role(object: &serde_json::Map<String, Value>) -> Option<String> {
    for name in ["role", "roles"] {
        match object.get(name) {
            Some(Value::String(value)) if !value.is_empty() => return Some(value.clone()),
            Some(Value::Array(values)) => {
                if let Some(value) = values.first().and_then(Value::as_str)
                    && !value.is_empty()
                {
                    return Some(value.to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn number_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number as i64))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
    use serde_json::json;

    const RSA_N: &str = "uSaZ1Bi8VphAToEANrYDZ-veau9T8cQbGpjVTm_zM2VFYe9yVSwstI0OZ1ta1LLO5FTaC3FLaxOO0b9hExMW1Nja0vqefg-m2bEcVCmLjTn-gQy36eu_poCA4FXGQET-EM8sKHrvSNi9HbJQPXaWurlu6pBbGeGnjxFAUo4dj0ZoAGp0kWis6ouf4R21yCwz6u9svUD0njCdPWZqkBIgTfJQIQHdRfVRg6duRZBJFFI548qvQjZ66jytD0q7lvkVdzlLWku5gu5BR-T75BMghTmMpGp7fMYWE0vfDJXJ3whCZsS3wQ3pNWpd-RJIewa6IytzlUyA8awWAdaw79LCaw";
    const RSA_E: &str = "AQAB";
    const RSA_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6InJzYS10ZXN0In0.eyJpc3MiOiJodHRwczovL3JzYS5leGFtcGxlLyIsImF1ZCI6ImhwYS1yc2EiLCJzdWIiOiJyc2EtdXNlciIsInRlbmFudF9pZCI6ImFjbWUiLCJyb2xlIjoiYWRtaW4iLCJlbWFpbCI6InJzYUBleGFtcGxlLmNvbSIsIm5hbWUiOiJSU0EgVXNlciIsImV4cCI6MjEwODg5MTE3OSwibm9uY2UiOiJyc2Etbm9uY2UifQ.YdXEwag6QRJAz-HaE1_30VwVz1nytYvjlT4V2kVzVGCIxk7xKQGXvLc2TYBs8b8CiJ0Ea5URwFCa-cWh28pvJL7CnI0A2MUX51P24Pzv75ly7ftlG-5vXvpQOmb5HUGJ3_E58iUnWnHLN3MoqHzG-NhvySJDYsH911ThLlghzr9HxbzwPP-LQiLeBjD20YR_oXews4IMm0gHoFmRIJpJ6JnhBDcz7nnpkPY-JA68sScsMKvYdP4spg-1bEpy0XE-dOhKAMyThjcJOSpt9SmdFscNjQfRoRzrKN4mElwPFiX6KKuDIfJUm86tOB6ueH_zrxf1rbSGYX0x40VghDBMAA";

    fn encode(value: &Value) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
    }

    fn ed_token(signing: &SigningKey, header: Value, claims: Value) -> String {
        let input = format!("{}.{}", encode(&header), encode(&claims));
        let signature = signing.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }

    fn ed_key(signing: &SigningKey, kid: &str) -> Jwk {
        Jwk {
            kid: Some(kid.to_string()),
            key: JwkKey::Ed25519(signing.verifying_key().to_bytes()),
        }
    }

    fn claims(nonce: &str) -> Value {
        let now = now_ms() / 1000;
        json!({
            "iss": "https://issuer.example/",
            "aud": "hpa",
            "sub": "user-1",
            "tenant_id": "acme",
            "role": "owner",
            "email": "user@example.com",
            "name": "User One",
            "exp": now + 3600,
            "nonce": nonce
        })
    }

    #[test]
    fn eddsa_verifies_and_rejects_bad_tokens() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let keys = vec![ed_key(&signing, "primary")];
        let token = ed_token(
            &signing,
            json!({"alg": "EdDSA", "typ": "JWT", "kid": "primary"}),
            claims("nonce-1"),
        );
        let principal = verify_jwt(
            &token,
            &keys,
            "https://issuer.example/",
            "hpa",
            Some("nonce-1"),
        )
        .unwrap();
        assert_eq!(principal.subject, "user-1");
        assert_eq!(principal.name.as_deref(), Some("User One"));
        assert!(
            verify_jwt(
                &token,
                &keys,
                "https://issuer.example/",
                "hpa",
                Some("other")
            )
            .is_err()
        );
        assert!(
            verify_jwt(
                &ed_token(
                    &signing,
                    json!({"alg": "HS256", "kid": "primary"}),
                    claims("nonce-1")
                ),
                &keys,
                "https://issuer.example/",
                "hpa",
                Some("nonce-1")
            )
            .is_err()
        );
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let bad = ed_token(
            &other,
            json!({"alg": "EdDSA", "kid": "primary"}),
            claims("nonce-1"),
        );
        assert!(
            verify_jwt(
                &bad,
                &keys,
                "https://issuer.example/",
                "hpa",
                Some("nonce-1")
            )
            .is_err()
        );
        let no_kid = ed_token(&signing, json!({"alg": "EdDSA"}), claims("nonce-1"));
        assert!(
            verify_jwt(
                &no_kid,
                &[keys[0].clone(), ed_key(&other, "secondary")],
                "https://issuer.example/",
                "hpa",
                None
            )
            .is_err()
        );
    }

    #[test]
    fn es256_verifies_and_rejects_malformed_signatures() {
        let rng = SystemRandom::new();
        let document =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key_pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, document.as_ref(), &rng)
                .unwrap();
        let public = key_pair.public_key().as_ref();
        assert_eq!(public.len(), 65);
        let mut x = [0u8; 32];
        let mut y = [0u8; 32];
        x.copy_from_slice(&public[1..33]);
        y.copy_from_slice(&public[33..]);
        let key = Jwk {
            kid: Some("ec".to_string()),
            key: JwkKey::Ec { x, y },
        };
        let input = format!(
            "{}.{}",
            encode(&json!({"alg": "ES256", "kid": "ec"})),
            encode(&claims("ec-nonce"))
        );
        let signature = key_pair.sign(&rng, input.as_bytes()).unwrap();
        let token = format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
        let principal = verify_jwt(
            &token,
            std::slice::from_ref(&key),
            "https://issuer.example/",
            "hpa",
            Some("ec-nonce"),
        )
        .unwrap();
        assert_eq!(principal.role, "owner");
        let mut parts: Vec<&str> = token.split('.').collect();
        parts[2] = "AAAA";
        let truncated = parts.join(".");
        assert!(
            verify_jwt(
                &truncated,
                &[key],
                "https://issuer.example/",
                "hpa",
                Some("ec-nonce")
            )
            .is_err()
        );
    }

    #[test]
    fn rsa_verifies_with_pkcs1_der() {
        let document = json!({"keys": [{
            "kty": "RSA",
            "kid": "rsa-test",
            "n": RSA_N,
            "e": RSA_E
        }]});
        let keys = parse_jwks(&document).unwrap();
        assert_eq!(keys.len(), 1);
        let principal = verify_jwt(
            RSA_TOKEN,
            &keys,
            "https://rsa.example/",
            "hpa-rsa",
            Some("rsa-nonce"),
        )
        .unwrap();
        assert_eq!(principal.subject, "rsa-user");
        assert_eq!(principal.name.as_deref(), Some("RSA User"));
        assert!(
            verify_jwt(
                RSA_TOKEN,
                &keys,
                "https://rsa.example/",
                "hpa-rsa",
                Some("wrong")
            )
            .is_err()
        );
        let mut parts: Vec<&str> = RSA_TOKEN.split('.').collect();
        parts[2] = "AAAA";
        assert!(
            verify_jwt(
                &parts.join("."),
                &keys,
                "https://rsa.example/",
                "hpa-rsa",
                Some("rsa-nonce")
            )
            .is_err()
        );
    }

    #[test]
    fn jwks_parsing_ignores_unknown_and_rejects_malformed_keys() {
        let document = json!({"keys": [
            {"kty": "oct", "k": "abc"},
            {"kty": "OKP", "crv": "X25519", "x": "abc"},
            {"kty": "EC", "crv": "P-384", "x": "abc", "y": "def"}
        ]});
        assert!(parse_jwks(&document).unwrap().is_empty());
        let missing_x = json!({"keys": [{"kty": "OKP", "crv": "Ed25519"}]});
        assert!(parse_jwks(&missing_x).is_err());
        let small = json!({"keys": [{
            "kty": "RSA",
            "n": "AQAB",
            "e": "AQAB"
        }]});
        assert!(parse_jwks(&small).is_err());
        let unsupported = json!({"keys": [
            {"kty": "RSA", "n": RSA_N, "e": RSA_E, "kid": "a"}
        ]});
        let key = parse_jwks(&unsupported).unwrap().remove(0);
        let token = ed_token(
            &SigningKey::from_bytes(&[7u8; 32]),
            json!({"alg": "EdDSA", "kid": "a"}),
            claims("nonce"),
        );
        assert!(verify_jwt(&token, &[key], "https://issuer.example/", "hpa", None).is_err());
    }
}
