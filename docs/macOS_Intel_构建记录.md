# FileM 0.1.4：macOS Intel 版本

构建日期：2026-09-13。目标为 Intel `x86_64-apple-darwin`，构建与验证主机为 Intel Core i5-8257U、macOS 15.7.9，使用 Rust 1.98.1 和 Node.js 25.6.1。

## 下载与安装

- [DMG 安装包](https://github.com/LiuMaoDou/openFile/releases/download/v0.1.4/FileM_0.1.4_macos-intel.dmg)：打开后将 `FileM.app` 拖到 Applications（应用程序）。
- [ZIP 应用包](https://github.com/LiuMaoDou/openFile/releases/download/v0.1.4/FileM-0.1.4-macos-intel.zip)：解压后将 `FileM.app` 移入“应用程序”。
- [SHA-256 校验文件](https://github.com/LiuMaoDou/openFile/releases/download/v0.1.4/SHA256SUMS-macos-intel.txt)。

使用者无需安装 Node.js、Rust、AnyTXT 或 Office。Mac 使用内置本地索引，Everything 加速仅用于 Windows。

应用使用临时签名（ad-hoc）封装完整性信息，没有 Apple Developer ID 签名，也没有公证。首次打开如果因无法验证开发者而受阻，在确认下载来源后，按 [Apple 官方说明](https://support.apple.com/zh-cn/102445) 到“系统设置 → 隐私与安全性 → 仍要打开”。安装包附有中文使用说明。

## 构建来源

工作区提交为 `a569db4140c60cced9739334dbf4d1fc49954c98`，与 v0.1.4 标签提交 `6912f4afa3c1baac9c9b36d4e94452bed59cf720` 相比只有 README 下载说明变化，程序源码和 Cargo.lock 相同。没有变更现有 v0.1.4 标签。

在 Intel Mac 上执行：

```sh
CARGO_BUILD_JOBS=2 npm run desktop:build -- --bundles app -- --locked
codesign --force --sign - --timestamp=none target/release/bundle/macos/FileM.app
codesign --verify --deep --strict --verbose=2 target/release/bundle/macos/FileM.app
```

默认 Rust 主机目标已核对为 `x86_64-apple-darwin`。使用 macOS `ditto` 制作 ZIP，`hdiutil` 制作 HFS+ / UDZO DMG；两者包含同一份 `FileM.app`、Applications 链接和中文使用说明。发布包与构建缓存不纳入源码版本控制。

## 本次验证

| 检查                             | 结果                                                                        |
| -------------------------------- | --------------------------------------------------------------------------- |
| TypeScript 与 Vite 生产构建      | 通过                                                                        |
| Tauri release 构建与 `.app` 打包 | 通过，应用版本为 0.1.4                                                      |
| 文件架构                         | `Mach-O 64-bit executable x86_64`，`lipo -archs` 仅返回 `x86_64`            |
| 动态库依赖                       | 仅 macOS 系统库与系统 Framework，不引用本机开发目录                         |
| 核心测试                         | 54 项通过，0 失败、0 忽略                                                   |
| 独立内容索引 smoke               | PDF/DOCX/XLSX/PPTX 解析、中英文搜索、文件夹开启与关闭清理通过               |
| 发布包中的实际可执行文件         | 四种文档解析均包含预期英文词和中文短语；ZIP 解压后重跑通过                  |
| ZIP                              | 解压后架构、二进制 SHA-256、bundle 完整性验证通过                           |
| DMG                              | `hdiutil verify`、只读挂载、版本、Applications 链接与 bundle 完整性验证通过 |
| 格式与差异检查                   | Rust 格式和 `git diff --check` 通过                                         |

## 校验值

```text
53119cfd8c39ffdf30449523a9869a49997e77ea6c82ec42b938042c7d84011c  FileM_0.1.4_macos-intel.dmg
556b2e1f7dff89140bb2f54f5c4ed64a3b6f61ba02892911da3da1bf2553859d  FileM-0.1.4-macos-intel.zip
```

DMG 为 8,588,654 字节；ZIP 为 6,640,186 字节。Mac 校验文件独立发布，保留原 Windows 包及其校验文件。

## 验证边界

本次验证覆盖编译、包结构、实际打包程序的内容解析和核心行为；未逐项验收原生窗口、目录选择器、首次下载后的 Gatekeeper 提示或所有旧版 macOS。旧版系统兼容性不能仅依据包里的最低版本声明判断。应用退出后不继续后台监听或索引。

这仍是 0.1.4 开发验证版。内容搜索不支持扫描版 PDF、图片 OCR、旧版 Office、WPS 专有格式或压缩包内容；数据库升级到版本 4 后，旧应用不能重新打开升级后的索引。
