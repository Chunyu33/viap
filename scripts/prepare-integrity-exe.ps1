param(
    [Parameter(Mandatory = $true)]
    [string]$Path
)

# NSIS 会把 Tauri 主程序中的 bundle 类型标记改为 NSS，签名必须覆盖安装后实际运行的字节。
$unknownMarker = [Text.Encoding]::ASCII.GetBytes('__TAURI_BUNDLE_TYPE_VAR_UNK')
$nsisMarker = [Text.Encoding]::ASCII.GetBytes('__TAURI_BUNDLE_TYPE_VAR_NSS')

if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "未找到待处理的 exe: $Path"
}

$bytes = [IO.File]::ReadAllBytes($Path)
$asciiContent = [Text.Encoding]::ASCII.GetString($bytes)
$markerText = '__TAURI_BUNDLE_TYPE_VAR_UNK'
$matchIndex = $asciiContent.IndexOf($markerText, [StringComparison]::Ordinal)

if ($matchIndex -lt 0) {
    throw "exe 中未找到 Tauri NSIS bundle 类型标记: $Path"
}
if ($asciiContent.IndexOf($markerText, $matchIndex + $markerText.Length, [StringComparison]::Ordinal) -ge 0) {
    throw "exe 中发现多个 Tauri bundle 类型标记，拒绝签名: $Path"
}

# 两个标记长度相同，只替换标识文本，不改变 PE 文件布局和文件长度。
for ($offset = 0; $offset -lt $nsisMarker.Length; $offset++) {
    $bytes[$matchIndex + $offset] = $nsisMarker[$offset]
}
[IO.File]::WriteAllBytes($Path, $bytes)
Write-Output "已将 NSIS bundle 标记写入: $Path"
