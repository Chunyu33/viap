//! 当前运行文件的完整性校验。
//!
//! 发布流程会为不同发行形态的 **原始 exe** 上传 Minisign 签名。校验时只下载签名文本，
//! 并让所有候选签名共享一次本地文件读取，避免大文件在机械盘上被重复读取。
//!
//! 这里校验的是「正在运行的 exe」的字节，所以签名对象必须是原始 exe ——
//! 安装包与 zip 的签名对应的字节和运行中的 exe 永远不可能相同，不能拿来校验。

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

/// 当前发行形态可能对应的原始 exe 资产后缀。
///
/// 便携版由 `--features portable` 单独构建，可以在编译期确定；
/// 标准版与 WebView2 离线版的可执行文件来自两次独立构建，运行期无法区分，
/// 因此只能把两个候选都试一遍（两者都是非便携构建，命中的那个即当前形态）。
fn signed_executable_suffixes() -> &'static [&'static str] {
    if cfg!(feature = "portable") {
        &["x64-portable.exe"]
    } else {
        &["x64.exe", "x64-offline-webview2.exe"]
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityStatus {
    Verified,
    Tampered,
    NetworkError,
    SignatureNotFound,
    /// 只下载到部分发行形态的签名，不足以对「未命中」下篡改的结论。
    SignatureIncomplete,
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

/// 解析 GitHub 上的签名文件。
///
/// Tauri signer 上传的 `.sig` 是“外层 Base64 + 内层 Minisign 文本”，而不是
/// `minisign_verify` 直接要求的四行文本；同时保留直接解析路径，兼容标准格式。
fn decode_signature_content(content: &str) -> Result<Signature, String> {
    let trimmed = content.trim();
    if let Ok(signature) = Signature::decode(trimmed) {
        return Ok(signature);
    }

    let decoded = STANDARD
        .decode(trimmed)
        .map_err(|error| format!("签名外层 Base64 解码失败: {error}"))?;
    let minisign_text = String::from_utf8(decoded)
        .map_err(|error| format!("签名内层文本编码无效: {error}"))?;
    Signature::decode(&minisign_text)
        .map_err(|error| format!("Minisign 签名内容解析失败: {error}"))
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

/// 一次收集的结果：成功下载的签名，以及该发行形态下缺失的资产名。
///
/// 缺哪些必须记下来：只有候选签名「一个都不缺」时，校验未命中才能断言篡改；
/// 否则运行的可能正是一个签名还没上传的发行形态。
struct CollectedSignatures {
    downloaded: Vec<DownloadedSignature>,
    missing: Vec<String>,
}

async fn collect_signatures(
    client: &reqwest::Client,
    tag: &str,
    suffixes: &[&str],
) -> Result<CollectedSignatures, SignatureFetchError> {
    let mut downloaded = Vec::with_capacity(suffixes.len());
    let mut missing = Vec::new();
    for suffix in suffixes {
        let asset_name = format!("viap_{tag}_{suffix}");
        match download_signature(client, tag, &asset_name).await? {
            Some(content) => downloaded.push(DownloadedSignature {
                asset_name,
                content,
            }),
            None => missing.push(asset_name),
        }
    }
    Ok(CollectedSignatures { downloaded, missing })
}

fn verify_local_file(
    path: &std::path::Path,
    public_key: &PublicKey,
    signatures: &[DownloadedSignature],
) -> Result<Option<String>, LocalVerificationError> {
    let decoded_signatures: Vec<(String, Signature)> = signatures
        .iter()
        .map(|signature| {
            decode_signature_content(&signature.content)
                .map(|decoded| (signature.asset_name.clone(), decoded))
                .map_err(LocalVerificationError::Signature)
        })
        .collect::<Result<_, _>>()?;

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

    let suffixes = signed_executable_suffixes();
    let collected = match collect_signatures(&client, &tag, suffixes).await {
        Ok(collected) => collected,
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

    let CollectedSignatures { downloaded, missing } = collected;

    // 该发行形态一个签名都没拿到：正常发布渠道必然带签名，说明当前版本不是官方构建，
    // 或发布流程漏传 —— 无论哪种都不能给出「安全」或「被篡改」的结论。
    if downloaded.is_empty() {
        return result(
            IntegrityStatus::SignatureNotFound,
            format!("未找到版本 {tag} 对应的程序签名文件，请确认当前版本来自官方发布渠道"),
            None,
        );
    }

    // exe 读取可能持续数秒，放入阻塞线程池避免机械盘校验阻塞 Tauri 异步运行时。
    let verification = match tauri::async_runtime::spawn_blocking(move || {
        verify_local_file(&executable_path, &public_key, &downloaded)
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
        // 候选签名一个不缺、却全部不匹配，才能断定本地 exe 被改过。
        Ok(None) if missing.is_empty() => result(
            IntegrityStatus::Tampered,
            "签名与当前程序内容不一致，当前程序可能已被篡改",
            None,
        ),
        // 还有候选没下载到，就不能排除「运行的正是一个缺签名的发行形态」，
        // 此时报篡改是误报，只能如实说明无法判断。
        Ok(None) => result(
            IntegrityStatus::SignatureIncomplete,
            format!(
                "当前 Release 缺少 {} 个发行形态的程序签名（{}），无法可靠判断当前程序是否被篡改",
                missing.len(),
                missing.join("、")
            ),
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

#[cfg(test)]
mod tests {
    use super::{decode_signature_content, signed_executable_suffixes};

    #[test]
    fn signature_candidates_are_raw_executables_only() {
        // 安装包 / 压缩包的签名对应的字节与运行中的 exe 不同，混进候选只会永远不命中，
        // 并把「签名缺失」掩盖成「校验不通过」。这条断言防的就是发布资产改名后再次跑偏。
        for suffix in signed_executable_suffixes() {
            assert!(
                suffix.ends_with(".exe"),
                "签名候选必须是原始 exe：{suffix}"
            );
            assert!(
                !suffix.contains("setup"),
                "安装包签名不能用于校验运行中的 exe：{suffix}"
            );
            assert!(
                !suffix.contains("zip"),
                "压缩包签名不能用于校验运行中的 exe：{suffix}"
            );
        }
    }

    #[test]
    fn signature_candidates_are_ordered_and_deduplicated() {
        let suffixes = signed_executable_suffixes();
        assert!(!suffixes.is_empty(), "至少要有一个签名候选");
        let mut seen = suffixes.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), suffixes.len(), "签名候选不应重复");
    }

    #[test]
    fn decodes_tauri_outer_base64_signature() {
        // 该样本对应 Tauri signer 生成的格式，防止把外层 Base64 当成 Minisign 文本。
        let encoded = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSS0dmRVJKUEtkWWZ1YWlRN09TSnFIV2pUZmp5WG5qNVRXbGtJeGZaQUpjL2ZFL3pSVUczNFBsd1orUWVQSTM4RTgrRzRSY1VTd29nRmNoaXZwOFFDUFdZZTdHUUh4MFFjPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzg0MTg3Mzg5CWZpbGU6dmlhcF92MS4xLjdfeDY0LmV4ZQp6RkhYUXczZlRJd1dGRTFDU3lrWDBqaFhtRFNHUVpnemtLMmIvN0lpd1pSak0yd0w5ZDFHbWY1Q2VqNmtZczRUeitmOTArejJtWVVmMFFSRFpjMEVEUT09Cg==";
        assert!(decode_signature_content(encoded).is_ok());
    }

    #[test]
    fn rejects_invalid_signature_content() {
        assert!(decode_signature_content("not-a-signature").is_err());
    }
}
