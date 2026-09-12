# FileM 0.1.0 Windows 构建记录

日期：2026-09-12。目标：Windows 11 x64，Rust 目标 `x86_64-pc-windows-msvc`。这是开发验证版，未做代码签名。

## 构建方式

在 macOS x86_64 上交叉编译，使用 Rust 1.98.1、cargo-xwin 0.23.1、LLVM / LLD 20.1.8、Microsoft Windows SDK 10.0.26100 和 NSIS 3.12。Windows 专用配置为 `src-tauri/tauri.windows.conf.json`。

```sh
npm ci
npm run windows:build
```

macOS / Linux 构建者还需安装 Windows MSVC Rust target、cargo-xwin、LLVM、LLD、NSIS；本工作区的交叉编译缓存保存在 `.tools/`。Windows 本机构建者使用 Rust MSVC 与 Visual Studio C++ 编译工具，脚本自动选择原生 Cargo。

安装器采用当前用户安装、中文界面，并内嵌 WebView2 安装引导程序；缺少 WebView2 时需要联网补齐。免安装版使用系统 WebView2。Rust/C 运行库设为静态链接，界面资源和数据库核心编入应用；使用者无需 Node.js、Rust、开发服务或编译工具。

`.cargo/config.toml` 启用完整 CRT 静态链接；打包脚本设置 `STATIC_VCRUNTIME=false`，避免 Tauri 的“静态 VC / 动态 UCRT”覆盖项与 cargo-xwin 冲突。该设置保留完整静态链接，不要求使用者安装 VC++ 运行库。依据为当前依赖源码及 [Tauri 的覆盖项实现](https://docs.rs/crate/tauri-build/2.6.3/source/src/static_vcruntime.rs)。

## 验证状态

- Windows 核心交叉编译检查通过。
- Windows 核心及其测试/示例目标的 Clippy 严格检查通过：`cargo-xwin clippy --locked -p filem-core --all-targets --target x86_64-pc-windows-msvc -- -D warnings`。这里检查了 Windows 测试代码，没有执行 Windows 测试程序。
- TypeScript 检查与 Vite 生产构建通过。
- 最终 Windows release 构建和 NSIS 安装器生成通过。应用程序为 13,118,976 字节的 PE32+ / AMD64 / Windows GUI；安装器为 5,004,345 字节。NSIS 引导壳自身为 PE32，所安装应用是 x64。
- PE 导入表仅含 13 个 Windows 系统 DLL，不依赖额外的 `VCRUNTIME`、`MSVCP` 或 `WebView2Loader.dll`；WebView2 浏览器运行时仍由系统提供。
- 安装包 `7z t` 及解包通过；解出的应用与免安装应用仅有 Tauri 包类型标记 `UNK` / `NSS` 的 3 字节差异，其余字节完全一致，符合依赖中 `tauri-utils/src/platform.rs` 的定义。
- 本次修改后再次执行 macOS 核心回归：20 项全部通过，无跳过项。Rust 格式和新增脚本/配置的 Prettier 检查通过。
- 当前主机没有 Windows 运行环境，未进行 Windows 原生安装、启动、目录选择或实际回收站测试。macOS 上已有的 20 项核心测试和界面验证见 [开发验证记录](FileM_开发验证记录.md)，不能替代 Windows 实机验收。

交叉链接提示 Microsoft CRT 的调试 PDB 未包含；这是第三方运行库的调试符号告警，链接成功，分发程序不需要这些 PDB。NSIS 在 macOS 上提示 `OUTPUTCHARSET` 不适用；安装脚本启用了 Unicode 和简体中文。工具同时明确提示交叉编译的兼容性边界，且本包未签名。

## 分发文件

- `releases/FileM_0.1.0_x64-setup.exe`：安装版。
- `releases/FileM-0.1.0-windows-x64-portable.zip`：完整解压后双击 `FileM.exe`。
- `releases/SHA256SUMS.txt`：安装包和免安装包的 SHA-256 校验值。

这些文件在本地生成，没有上传或发布到外部服务。

## Windows 试用核对

使用独立测试文件夹，确认安装或解压后可双击启动、添加范围、按扩展名筛选、文本预览、系统打开和定位；再用两份可丢弃文件核对多选回收及从系统回收站恢复。长路径、UNC、junction、OneDrive、权限变化、文件锁及不可回收卷仍需专项验收。
