//! 当前运行文件的完整性校验。
//!
//! 发布流程会为不同发行形态的 exe 上传 Minisign 签名。校验时只下载签名文本，
//! 并让所有候选签名共享一次本地文件读取，避免大文件在机械盘上被重复读取。

use std::fs::File;
use std::io::Read;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use minisign_verify::{Error as MinisignError, PublicKey, Signature};
use reqwest::StatusCode;
use serde::Serialize;

// 该公钥与 tauri.conf.json 的 updater.pubkey 必须保持一致，避免完整性校验使用另一把密钥。
const UPDATER_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDYxOURGMjI0MTFGMTE5NEEKUldSS0dmRVJKUEtkWVlEUjV1d3dvdVg4S2p6VUFLN1Q4enhraVVkZ01tcDU5MXpVRGEyNjN5R0UK";
const GITHUB_RELEASE_BASE_URL: &str = "https://github.com/Chunyu33/viap/releases/download";
const SIGNED_EXECUTABLE_SUFFIXES: [&str; 3] =
    ["x64.exe", "x64-offline-webview2.exe", "x64-portable.exe"];

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityStatus {
    Verified,
    Tampered,
    NetworkError,
    SignatureNotFound,
    SignatureInvalid,
    LocalFileError,
    ConfigurationError,
}

#[derive(Debug, Serialize)]
pub struct IntegrityCheckResult {
    pub status: IntegrityStatus,
    pub message: String,
    pub asset_name: Option<String>,
}

struct DownloadedSignature {
    asset_name: String,
    content: String,
}

enum SignatureFetchError {
    Network(String),
    Remote(String),
}

enum LocalVerificationError {
    File(String),
    Signature(String),
}

fn result(
    status: IntegrityStatus,
    message: impl Into<String>,
    asset_name: Option<String>,
) -> IntegrityCheckResult {
    IntegrityCheckResult {
        status,
        message: message.into(),
        asset_name,
    }
}

fn load_public_key() -> Result<PublicKey, String> {
    let key_bytes = STANDARD
        .decode(UPDATER_PUBLIC_KEY)
        .map_err(|error| format!("公钥 Base64 解码失败: {error}"))?;
    let key_text =
        String::from_utf8(key_bytes).map_err(|error| format!("公钥文本编码无效: {error}"))?;
    PublicKey::decode(&key_text).map_err(|error| format!("Minisign 公钥解析失败: {error}"))
}

async fn download_signature(
    client: &reqwest::Client,
    tag: &str,
    asset_name: &str,
) -> Result<Option<String>, SignatureFetchError> {
    let url = format!("{GITHUB_RELEASE_BASE_URL}/{tag}/{asset_name}.sig");
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| SignatureFetchError::Network(error.to_string()))?;

    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(SignatureFetchError::Remote(format!(
            "GitHub 返回 HTTP {}",
            response.status()
        )));
    }

    response
        .text()
        .await
        .map(Some)
        .map_err(|error| SignatureFetchError::Network(error.to_string()))
}

async fn collect_signatures(
    client: &reqwest::Client,
    tag: &str,
) -> Result<Vec<DownloadedSignature>, SignatureFetchError> {
    let mut signatures = Vec::with_capacity(SIGNED_EXECUTABLE_SUFFIXES.len());
    for suffix in SIGNED_EXECUTABLE_SUFFIXES {
        let asset_name = format!("viap_{tag}_{suffix}");
        if let Some(content) = download_signature(client, tag, &asset_name).await? {
            signatures.push(DownloadedSignature {
                asset_name,
                content,
            });
        }
    }
    Ok(signatures)
}

fn verify_local_file(
    path: &std::path::Path,
    public_key: &PublicKey,
    signatures: &[DownloadedSignature],
) -> Result<Option<String>, LocalVerificationError> {
    let decoded_signatures: Vec<(String, Signature)> = signatures
        .iter()
        .filter_map(|signature| {
            Signature::decode(&signature.content)
                .ok()
                .map(|decoded| (signature.asset_name.clone(), decoded))
        })
        .collect();

    if decoded_signatures.len() != signatures.len() {
        return Err(LocalVerificationError::Signature(
            "GitHub 签名文件格式无效".to_string(),
        ));
    }

    // 所有 verifier 借用同一组签名，下面只需把本地 exe 读取一遍即可完成多候选校验。
    let mut verifiers = Vec::with_capacity(decoded_signatures.len());
    for (asset_name, signature) in &decoded_signatures {
        let verifier = public_key.verify_stream(signature).map_err(|error| {
            LocalVerificationError::Signature(format!(
                "签名 {} 不支持流式校验: {error}",
                asset_name
            ))
        })?;
        verifiers.push((asset_name, verifier));
    }

    let mut file = File::open(path)
        .map_err(|error| LocalVerificationError::File(format!("无法读取当前程序文件: {error}")))?;
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let bytes_read = file.read(&mut buffer).map_err(|error| {
            LocalVerificationError::File(format!("读取当前程序文件失败: {error}"))
        })?;
        if bytes_read == 0 {
            break;
        }
        for (_, verifier) in &mut verifiers {
            verifier.update(&buffer[..bytes_read]);
        }
    }

    for (asset_name, verifier) in &mut verifiers {
        match verifier.finalize() {
            Ok(()) => return Ok(Some((*asset_name).clone())),
            Err(MinisignError::InvalidSignature) => {}
            Err(error) => {
                return Err(LocalVerificationError::Signature(format!(
                    "签名 {} 校验失败: {error}",
                    asset_name
                )))
            }
        }
    }
    Ok(None)
}

/// 从当前版本 GitHub Release 下载签名并校验运行中的 exe。
pub async fn verify_file_integrity(app_handle: tauri::AppHandle) -> IntegrityCheckResult {
    let public_key = match load_public_key() {
        Ok(key) => key,
        Err(error) => return result(IntegrityStatus::ConfigurationError, error, None),
    };
    let executable_path = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return result(
                IntegrityStatus::LocalFileError,
                format!("无法定位当前程序文件: {error}"),
                None,
            )
        }
    };
    let version = app_handle.package_info().version.to_string();
    let tag = format!("v{version}");
    let client = match reqwest::Client::builder()
        .user_agent(format!("Viap/{version}"))
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return result(
                IntegrityStatus::ConfigurationError,
                format!("创建网络校验服务失败: {error}"),
                None,
            )
        }
    };

    let signatures = match collect_signatures(&client, &tag).await {
        Ok(signatures) => signatures,
        Err(SignatureFetchError::Network(error)) => {
            return result(
                IntegrityStatus::NetworkError,
                format!("无法连接 GitHub，暂时无法完成校验，请稍后重试: {error}"),
                None,
            )
        }
        Err(SignatureFetchError::Remote(error)) => {
            return result(
                IntegrityStatus::NetworkError,
                format!("GitHub 暂时无法提供校验文件，请稍后重试: {error}"),
                None,
            )
        }
    };

    if signatures.is_empty() {
        return result(
            IntegrityStatus::SignatureNotFound,
            format!("未找到版本 {tag} 对应的程序签名文件，请确认当前版本来自官方发布渠道"),
            None,
        );
    }
    // 三种发行形态缺一时无法确认当前 exe 对应哪个构建，必须避免把缺失签名误报为篡改。
    if signatures.len() != SIGNED_EXECUTABLE_SUFFIXES.len() {
        return result(
            IntegrityStatus::SignatureNotFound,
            "当前 Release 的完整性签名不完整，无法可靠判断当前程序是否被篡改",
            None,
        );
    }

    let valid_signature_count = signatures
        .iter()
        .filter(|signature| Signature::decode(&signature.content).is_ok())
        .count();
    if valid_signature_count != signatures.len() {
        return result(
            IntegrityStatus::SignatureInvalid,
            "GitHub 签名文件解析失败，无法完成完整性校验",
            None,
        );
    }

    // exe 读取可能持续数秒，放入阻塞线程池避免机械盘校验阻塞 Tauri 异步运行时。
    let verification = match tauri::async_runtime::spawn_blocking(move || {
        verify_local_file(&executable_path, &public_key, &signatures)
    })
    .await
    {
        Ok(verification) => verification,
        Err(error) => {
            return result(
                IntegrityStatus::LocalFileError,
                format!("完整性校验线程异常，请稍后重试: {error}"),
                None,
            )
        }
    };

    match verification {
        Ok(Some(asset_name)) => result(
            IntegrityStatus::Verified,
            "文件完整性校验通过，当前程序安全",
            Some(asset_name),
        ),
        Ok(None) => result(
            IntegrityStatus::Tampered,
            "签名与当前程序内容不一致，当前程序可能已被篡改",
            None,
        ),
        Err(LocalVerificationError::File(error)) => {
            result(IntegrityStatus::LocalFileError, error, None)
        }
        Err(LocalVerificationError::Signature(error)) => {
            result(IntegrityStatus::SignatureInvalid, error, None)
        }
    }
}
