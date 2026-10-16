# 把 Rust 侧的构建前置摆到 PATH 与 LIBCLANG_PATH 上。
#
# 用法（仓库根目录，注意前面有个点：点源才能把环境变量留在当前会话）：
#     . .\rust\setup-env.ps1
#     cargo test -p gd3-core -p gd3-net -p gd3-servers
#     uvx maturin develop --uv --manifest-path rust/crates/gd3-py/Cargo.toml
#
# 为什么需要这些：`wreq` → `btls-sys` 要现编 BoringSSL（cmake + nasm），
# bindgen 要 libclang。缺任何一个都会在 `btls-sys` 处炸掉，
# 而 rustc 的报错不会告诉你到底缺什么——所以这个脚本负责把话说清楚。

[CmdletBinding()]
param(
    # 便携版 cmake / nasm 放在这里（默认 ~/.local/tools/{cmake,nasm}）
    [string]$ToolsRoot = (Join-Path $HOME ".local\tools"),
    # 提供 libclang 与 PYO3_PYTHON 的虚拟环境
    [string]$Venv = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Venv) { $Venv = Join-Path $repoRoot ".venv" }

$missing = @()

# cmake / nasm：优先用 $ToolsRoot 下的便携版，其次看 PATH 上有没有
$tools = @(
    [pscustomobject]@{ Name = "cmake"; Sub = "cmake\bin"; Exe = "cmake.exe" },
    [pscustomobject]@{ Name = "nasm"; Sub = "nasm"; Exe = "nasm.exe" }
)
foreach ($tool in $tools) {
    $bundled = Join-Path $ToolsRoot $tool.Sub
    if (Test-Path (Join-Path $bundled $tool.Exe)) {
        if ($env:PATH -notlike "*$bundled*") { $env:PATH = "$bundled;$env:PATH" }
        Write-Host "  $($tool.Name.PadRight(8)) -> $bundled"
    }
    elseif (Get-Command $tool.Name -ErrorAction SilentlyContinue) {
        Write-Host "  $($tool.Name.PadRight(8)) -> PATH 上已有"
    }
    else {
        $missing += "$($tool.Name)：放到 $bundled，或装进 PATH"
    }
}

# libclang：bindgen 用它解析 BoringSSL 头文件
$libclang = Join-Path $Venv "Lib\site-packages\clang\native"
if (Test-Path (Join-Path $libclang "libclang.dll")) {
    $env:LIBCLANG_PATH = $libclang
    Write-Host "  libclang -> $libclang"
}
else {
    $missing += "libclang：$Venv 里没有 clang 包（uv add --dev clang）"
}

# PyO3 要找到装了 gd3-py 的解释器；maturin/uv 认 VIRTUAL_ENV
$python = Join-Path $Venv "Scripts\python.exe"
if (Test-Path $python) {
    $env:PYO3_PYTHON = $python
    $env:VIRTUAL_ENV = $Venv
    Write-Host "  python   -> $python"
}
else {
    $missing += "虚拟环境：$Venv 不存在（先在仓库根跑 uv sync）"
}

if ($missing.Count -gt 0) {
    Write-Warning ("还缺构建前置，只有 gd3-core / gd3-servers 能编，gd3-net 与 gd3-py 会在 btls-sys 处失败：`n  - " +
        ($missing -join "`n  - "))
}
else {
    Write-Host "Rust 构建环境就绪。"
}
