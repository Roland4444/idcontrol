# Download ONNX PP-OCRv5 detection and East-Slavic Cyrillic recognition models.
# Run in PowerShell in project root: powershell -ExecutionPolicy Bypass -File .\scripts\install-local-ocr-models.ps1
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$folder = Join-Path $root 'models\ocr'
New-Item -ItemType Directory -Force -Path $folder | Out-Null
$files = @(
    @{ Path='det.onnx'; Url='https://huggingface.co/monkt/paddleocr-onnx/resolve/main/detection/v5/det.onnx'; Sha='61824840edf6e74581898930b8091b1b2318f4b2705a2e8a40ad3de7ac480133'; Min=80000000 },
    @{ Path='rec.onnx'; Url='https://huggingface.co/monkt/paddleocr-onnx/resolve/main/languages/eslav/rec.onnx'; Sha='dc6bf0e855247decce214ba6dae5bc135fa0ad725a5918a7fcfb59fad6c9cdee'; Min=7000000 },
    @{ Path='dict.txt'; Url='https://huggingface.co/monkt/paddleocr-onnx/resolve/main/languages/eslav/dict.txt'; Sha=''; Min=1000 }
)
foreach ($entry in $files) {
    $dest = Join-Path $folder $entry.Path
    $valid = $false
    if (Test-Path $dest) {
        $size = (Get-Item $dest).Length
        $hash = (Get-FileHash $dest -Algorithm SHA256).Hash.ToLowerInvariant()
        $valid = ($size -ge $entry.Min) -and (!$entry.Sha -or $hash -eq $entry.Sha)
    }
    if ($valid) { Write-Host "OK: $($entry.Path)"; continue }
    Write-Host "Downloading $($entry.Path): $($entry.Url)"
    $temp = "$dest.partial"
    try {
        Invoke-WebRequest -Uri $entry.Url -OutFile $temp -MaximumRedirection 10
        if ((Get-Item $temp).Length -lt $entry.Min) { throw "Downloaded content too small (possible HTML/LFS pointer)" }
        if ($entry.Sha) {
            $hash = (Get-FileHash $temp -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($hash -ne $entry.Sha) { throw "SHA-256 mismatch for $($entry.Path): $hash" }
        }
        Move-Item -Force $temp $dest
        Write-Host "OK: $($entry.Path)"
    } finally { if (Test-Path $temp) { Remove-Item $temp -Force } }
}
Write-Host "Done. Verify: cargo run -- --check-local-models"
