use anyhow::{Context, bail};
use predict_fun_sdk::{AuthRequest, Environment, PredictClient, PredictClientConfig};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use sha3::{Digest, Keccak256};
use std::env;

fn strip_0x(s: &str) -> &str {
    s.strip_prefix("0x").unwrap_or(s)
}

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn private_key_to_address(private_key_hex: &str) -> anyhow::Result<String> {
    let pk_bytes = hex::decode(strip_0x(private_key_hex)).context("invalid private key hex")?;
    let secret_key = SecretKey::from_slice(&pk_bytes).context("invalid private key bytes")?;
    let secp = Secp256k1::new();
    let pubkey = PublicKey::from_secret_key(&secp, &secret_key);
    let uncompressed = pubkey.serialize_uncompressed();
    let hash = keccak256(&uncompressed[1..]);
    Ok(format!("0x{}", hex::encode(&hash[12..])))
}

fn sign_personal_message(private_key_hex: &str, message: &str) -> anyhow::Result<String> {
    let pk_bytes = hex::decode(strip_0x(private_key_hex)).context("invalid private key hex")?;
    let secret_key = SecretKey::from_slice(&pk_bytes).context("invalid private key bytes")?;
    let secp = Secp256k1::new();

    let prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
    let mut payload = Vec::with_capacity(prefix.len() + message.len());
    payload.extend_from_slice(prefix.as_bytes());
    payload.extend_from_slice(message.as_bytes());
    let digest = keccak256(&payload);

    let msg = Message::from_digest_slice(&digest).context("failed to create secp256k1 message")?;
    let sig = secp.sign_ecdsa_recoverable(&msg, &secret_key);
    let (recovery_id, compact_sig) = sig.serialize_compact();
    let v = (recovery_id.to_i32() as u8) + 27;

    Ok(format!("0x{}{:02x}", hex::encode(compact_sig), v))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = env::var("PREDICT_API_KEY").context("missing PREDICT_API_KEY")?;
    let private_key = env::var("PREDICT_PRIVATE_KEY")
        .or_else(|_| env::var("PREDICT_SIGNER"))
        .context("missing PREDICT_PRIVATE_KEY (or PREDICT_SIGNER if you put the private key there)")?;
    let base_url = env::var("PREDICT_BASE_URL").unwrap_or_else(|_| Environment::Mainnet.base_url().to_string());

    let client = PredictClient::new(PredictClientConfig {
        base_url,
        api_key: Some(api_key),
        ..Default::default()
    })?;

    let auth_message_response = client.get_auth_message().await?;
    let message = auth_message_response
        .get("data")
        .and_then(|d| d.get("message"))
        .and_then(|m| m.as_str())
        .context("auth message not found in response")?;

    let signer = private_key_to_address(&private_key)?;
    let signature = sign_personal_message(&private_key, message)?;

    let jwt_response = client
        .get_jwt_with_valid_signature(&AuthRequest {
            signer: signer.clone(),
            signature,
            message: message.to_string(),
        })
        .await
        .context("JWT request failed")?;

    let token = jwt_response
        .get("data")
        .and_then(|d| d.get("token"))
        .and_then(|t| t.as_str())
        .context("token missing in JWT response")?
        .to_string();

    if token.is_empty() {
        bail!("received empty JWT token");
    }

    client.set_jwt_token(token)?;
    let account = client.get_connected_account().await.context("connected account request failed")?;
    let account_addr = account
        .get("data")
        .and_then(|d| d.get("wallet"))
        .and_then(|w| w.get("address"))
        .and_then(|a| a.as_str())
        .unwrap_or("<unknown>");

    println!("jwt flow: ok");
    println!("signer: {}", signer);
    println!("connected account: {}", account_addr);

    Ok(())
}
