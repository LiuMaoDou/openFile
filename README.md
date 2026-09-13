# openFile（FileM）

按用户指定的盘或文件夹聚合相同扩展名文件。索引、筛选和隐藏只改变本地视图，源文件保持原位。

当前版本：**0.1.6 开发验证版**。Rust + SQLite 核心、React 界面与 Tauri 2 桌面壳已实现；Windows 实机验证仍待执行，不能视为 M0/M1 全量验收通过。

## Intel Mac 直接运行

**[下载 macOS Intel DMG](https://github.com/LiuMaoDou/openFile/releases/download/v0.1.6/FileM_0.1.6_macos-intel.dmg)** · **[下载 macOS Intel ZIP](https://github.com/LiuMaoDou/openFile/releases/download/v0.1.6/FileM-0.1.6-macos-intel.zip)** · [Mac 校验值](https://github.com/LiuMaoDou/openFile/releases/download/v0.1.6/SHA256SUMS.txt)

适用于 Intel（x86_64）Mac。打开 DMG，将 `FileM.app` 拖入 Applications（应用程序）；也可解压 ZIP 后移入“应用程序”。无需安装 Node.js、Rust、AnyTXT 或 Office。Mac 使用内置本地索引，Everything 加速仅适用于 Windows。

此包未使用 Apple Developer ID 签名，也未经过公证。首次打开若因无法验证开发者而被阻止，在确认下载来源后，可按 [Apple 官方说明](https://support.apple.com/zh-cn/102445)，到“系统设置 → 隐私与安全性 → 仍要打开”。构建与验证范围见 [0.1.6 主题更新说明](https://github.com/LiuMaoDou/openFile/releases/tag/v0.1.6)。

## Windows 直接运行

**[下载 Windows 安装版](https://github.com/LiuMaoDou/openFile/releases/download/v0.1.6/FileM_0.1.6_x64-setup.exe)** · **[下载免安装 ZIP](https://github.com/LiuMaoDou/openFile/releases/download/v0.1.6/FileM-0.1.6-windows-x64-portable.zip)** · [版本说明与校验值](https://github.com/LiuMaoDou/openFile/releases/tag/v0.1.6)

Windows 11 x64 用户可运行 `FileM_0.1.6_x64-setup.exe`，或完整解压 `FileM-0.1.6-windows-x64-portable.zip` 后双击 `FileM.exe`。使用者无需安装 Node.js、Rust 或 Visual Studio。免安装包使用系统 WebView2；安装版会在缺少该运行时时联网安装。使用步骤和版本边界见 [Windows 使用说明](docs/Windows_使用说明.md) 与 [0.1.4 内容搜索更新说明](docs/Windows_0.1.4_更新说明.md)；此前修复见 [0.1.3 修复与验证记录](docs/Windows_0.1.3_修复记录.md) 和 [代码复查](docs/Windows_0.1.3_代码复查.md)。分发包通过 GitHub Releases 下载，构建产物不纳入源码版本控制。Release 页面中的 Source code 是源码压缩包，不能直接双击运行；Packages 栏目不用于此应用的安装包分发。

## 外观主题

在左下角 **外观** 选择 **浅色 / 暗色 / 系统**。默认跟随系统，并随系统外观变化即时切换；手动选择会保存在本机，重启后继续使用。Windows 与 macOS 使用相同的主题，覆盖筛选、列表、文件详情、内容索引和操作弹窗。

## 本地运行

需要 Node.js 22+、Rust stable 和平台编译工具。当前工作区已安装项目私有 Rust，脚本会自动使用 `.tools/cargo` 与 `.tools/rustup`，未修改系统 PATH；其他机器使用已有的 Rust 工具链。

```sh
npm ci
npm run dev
```

打开 http://127.0.0.1:5178 。浏览器开发入口通过本机 Rust 服务访问文件系统，支持输入本机文件夹完整路径；它不是纯浏览器文件系统方案。默认只监听本机回环地址，Rust API 需要每次启动生成的鉴权令牌。

Tauri 桌面运行支持系统目录选择器，直接调用 Rust 核心：

```sh
npm run desktop
```

两种开发入口使用相同的前端端口，切换前先用 Ctrl+C 停止上一种入口。

```sh
npm run desktop:build
# macOS 只构建 .app，跳过 DMG
npm run desktop:build -- --bundles app
```

Windows x64 安装包的构建命令：

```sh
npm run windows:build
```

在 Windows 开发机上使用 Rust MSVC 工具链和 Visual Studio C++ 编译工具；在 macOS / Linux 上使用 `cargo-xwin`、LLVM、LLD 与 NSIS。脚本固定目标为 `x86_64-pc-windows-msvc`，读取 Windows 专用安装配置。安装包输出到 `target/x86_64-pc-windows-msvc/release/bundle/nsis/`。这些编译工具仅构建者需要。

## 已实现

- 添加、移除和编辑监视文件夹，递归开关、独立排除规则；最多 32 个文件夹。
- 真实文件元数据扫描、SQLite 持久化、启动后全量核对；无法读取的子树保留旧索引。
- 父子文件夹独立成员关系；多个文件夹引用同一路径时聚合列表只显示一条，默认显示最内层有效文件夹。
- 扩展名归一化、类型分组、名称/路径搜索、文件夹/大小/时间筛选、排序；200 条分页与页内虚拟滚动。
- 多选、Shift 连选、本页全选、跨页保留选择；批量隐藏、恢复与移动。
- 单文件/批量删除到系统回收站：冻结选择、原路径确认、执行前复核、逐项结果、停止后续删除、后台处理和最近 10 批操作记录；每批最多 1000 个文件。
- 文件详情、原目录、显式文本预览（最多 64 KiB）、复制路径、系统打开、定位文件。
- 原生文件事件监听及文件夹重新核对，手动刷新、取消；文件夹离线时保留元数据并限制内容读取。
- 明确点击后才创建和索引的独立示例资料。

## 扫描状态与类型统计

列表上方持续显示扫描状态、完成时间或需要处理的问题；左侧每个监控文件夹显示状态和文件数。选中不同监控文件夹，类型和后缀列表只统计这个文件夹，隐藏零数量分类，并清除旧的类型筛选。

## Everything SDK（Windows 可选）

安装并运行 Everything 1.4 标准版（非 Lite），等待 Everything 完成自身索引，再勾选列表上方的 **Everything 加速**。官方 SDK 以静态库方式编译到 Windows 应用中，不需要另放 DLL。它查询正在运行的 Everything，不能单独替代 Everything 服务和应用。

初次添加文件夹可先导入最多 2,000 个候选结果，再由本地扫描完整核对。文件名/路径搜索可使用 Everything 的索引；只接受已添加文件夹内、符合排除规则且通过实际文件检查的结果。兼容 Windows 完整名称与 8.3 短名称混用的文件夹索引。SDK 未连接、超时、正在处理其他请求、命中超过 50,000 项，或 SDK 没命中但本地有结果时，使用本地 SQLite 搜索。搜索文本按字面量处理，尚不暴露 Everything 的高级搜索语法或正文搜索。

macOS 使用本地索引。Everything 的候选路径、授权检查和回退有测试覆盖；Windows CI 已通过与 Everything 1.4.1.1032 的真实 IPC 查询、中文路径及短名称混用检查。来源与许可见 [SDK 目录](crates/filem-core/vendor/everything/SOURCE.md) 和 [官方 SDK 文档](https://www.voidtools.com/support/everything/sdk/)。

## 本地内容搜索（0.1.4）

在文件夹设置中勾选 **启用内容索引**，或展开列表上方的 **内容索引**，为指定文件夹点击 **开启内容索引**。等待文字提取完成后，把搜索模式切换为 **文件内容** 或 **文件名 + 内容**。结果显示命中片段与高亮，点击文件可预览索引文字。无需安装 AnyTXT、Everything、Java 或 Office。

支持 TXT/Markdown/常见代码与配置文本、带文字层的 PDF、DOCX、XLSX、PPTX。文本解码支持 UTF-8、带 BOM 的 UTF-16 和 GBK。中文单字、短词以及中英文混合短语按连续字面量匹配；不把空格解释为 AND，不提供自然语言问答、通配符或正则。英文不区分大小写。

每个文件夹独立开启、暂停、继续、关闭或重建。状态显示完成、待处理、失败、跳过和截断数，失败文件可展开查看原因。仅提取开启文件夹中的文件；关闭后清理无其他开启文件夹引用的内容。正文与 FTS5 存在同一 SQLite 中，文件修改、移动、删除后同步失效或清理，避免旧内容与新路径混用。文件事件监听仍沿用元数据扫描的刷新机制，应用关闭期间不会后台索引。

首版边界：单文件最大 64 MiB，最多索引 2 MiB UTF-8 文字，截断会标明；解析进程有 30 秒时限，Windows 使用 512 MiB 内存上限、macOS 按实际内存占用监测并停止超限进程。扫描版 PDF/图片 OCR、旧版 DOC/XLS/PPT、WPS 专有格式和压缩包内容尚未支持。部分特殊 PDF 字体、损坏/加密文件可能提取失败，以逐项状态为准。

3 个及以上字符使用 SQLite FTS5 trigram 索引；1～2 个字符在已保存文字中检索，文档量大时会较慢。内容索引保存在本机应用数据目录，不加密；空间取决于提取文字与索引大小。关闭会删除逻辑记录，不承诺从 SQLite 空闲页、WAL 或备份中安全擦除正文。数据库升级为版本 4，旧应用不能直接打开升级后的数据库。

开发验证：`npm test` 覆盖内容索引与原有文件操作；`node scripts/cargo.mjs run -p filem-core --example content_smoke` 使用仓库内合成文档验证独立解析进程、中文全文查询及关闭清理。CI 在 Windows/macOS 执行相同流程。

## 批量移动

勾选多个文件 → **移动到…** → 选择目标文件夹 → 核对清单 → **确认移动**。已有同名文件、同一批次的重名项、身份变化和原地移动会跳过，不覆盖目标文件。支持停止后续移动、后台处理及工具栏的 **移动记录**。

移动期间暂停扫描，完成后刷新相关索引。移动到已监控位置时保留文件 ID；移出全部监控文件夹后从列表移除，实际文件在目标目录。Windows 使用支持跨盘的原生移动，macOS 跨卷复制分支校验文件内容后再移除原文件。操作中断或结果不明时请先核对两个位置，不会自动重试。仅移动普通文件，不移动目录、链接或云占位文件。

## 数据位置

浏览器开发模式默认写入 `.filem/index/index.sqlite`；示例文件在 `.filem/示例资料`。可用 `FILEM_DATA_DIR` 指定独立的开发数据目录。

桌面模式使用 Tauri 的应用本地数据目录，应用标识为 `local.filem.desktop`；它与浏览器开发数据独立。任何模式均不会自动扫描用户目录或整机磁盘。

索引保存原生路径编码、文件身份和目录引用，前端传递文件 ID。文本预览会检查根目录授权、当前文件身份、大小和修改时间，再读取内容；拒绝符号链接与已识别的云占位文件。

## 删除文件

勾选文件后点击底部 **删除文件**，或在详情中点击 **删除此文件**。核对原路径后点击 **移入回收站**。它会处理真实文件；“从视图隐藏”只影响列表。删除确认清单有效期 10 分钟，每份清单只能提交一次；文件在预览后变化或已离开有效文件夹时会跳过并说明原因。

执行中可以停止后续文件或转到后台，在工具栏的 **删除记录** 中查看结果。停止不会撤销已经完成的文件。成功项从索引和选择中移除，失败项保留；中断或无法确认的项目显示“需要核对”，不会自动重试。

macOS 使用 `NSFileManager`，Windows 使用 STA `IFileOperation` 并要求回收，均不提供永久删除回退。macOS 已用独立测试文件验证实际废纸篓内容和恢复；Windows CI 已通过对独立临时文件的原生回收测试，用户桌面与其他磁盘仍需验收。当前仅处理普通文件，不删除目录、链接、占位文件；macOS 非 UTF-8 路径暂不支持删除。

恢复请在系统回收站操作。macOS 部分系统不显示“放回原处”，可手动拖回；FileM 不提供自动撤销按钮。这是所用原生方法的已知行为，见 [trash 的 macOS 接口说明](https://docs.rs/trash/latest/trash/macos/enum.DeleteMethod.html)。文件复核与系统路径操作之间仍有并发竞态窗口，不能保证抵御其他进程在最后一刻替换路径。

## 验证

```sh
npm test
npm run build
npm run check:rust
node scripts/cargo.mjs clippy --workspace --all-targets -- -D warnings
node scripts/cargo.mjs fmt --all -- --check
npm run bench:index
```

核心集成测试涵盖文件夹递归与排除、重叠文件夹、隐藏持久化、深层重启补扫、文件替换、离线、排序分页、目录移动、原生监听、符号链接逃逸以及损坏数据库保护。测试与基准只创建临时资料，不操作用户源文件。

删除测试另覆盖只读预览、去重、重叠文件夹索引清理、过期、重复提交、变化/替换文件、文件夹移除、部分失败、停止、中断恢复和无法确认的系统结果。0.1.3 在 macOS 上共 46 项核心测试通过；该版本的统计、移动与 Everything 回退证据见 [0.1.3 验证记录](docs/Windows_0.1.3_修复记录.md)，原有功能及十万文件基准见 [开发验证记录](docs/FileM_开发验证记录.md)。

`.github/workflows/check.yml` 执行 Windows/macOS 构建、核心测试、格式与严格 Clippy 检查。Windows 另用隔离的临时目录运行真实 Everything IPC、批量移动及回收站测试；具体提交的状态以 [GitHub Actions](https://github.com/LiuMaoDou/openFile/actions) 为准。

## 当前边界与下一阶段

- 监听事件合并后触发整个文件夹的重新核对，尚未实现按路径增量更新、USN/MFT 加速或离线自动重连。
- 首版使用路径型遍历与单 SQLite 连接，取消在批次间生效；巨大单目录、百万文件、多文件夹同时写入仍需压力验证。目录重命名会重建路径条目，隐藏状态不会跟随重命名。
- 未建立内容索引时，直接文本预览按 UTF-8 解码；内容索引支持 GBK 和 PDF/Office 文字预览。尚未实现缩略图、文档原版式预览和手动编码选择。未跟随目录链接，也不会主动下载云占位内容。
- 尚未实现复制、重命名、Windows 原生右键菜单、拖放导出、重复文件与版本候选。操作预览与日志已覆盖回收站删除和批量移动。
- Windows 的长路径、UNC、junction、OneDrive、权限变化、不同文件系统、实际目录选择和系统打开需要 Windows 实机验收。当前 macOS 编译与浏览器验证不能替代这些检查。

下一开发批次依次完成 Windows M0 与回收站实机验证、文件夹监听增量更新与恢复、查询快照，再扩展复制和重命名。

## 设计与计划

- [方案审查与修订](docs/FileM_方案审查与修订.md)
- [开发计划与验收标准](docs/FileM_开发计划.md)
- [原始需求](docs/FileM_原始需求.md)
- [视觉约定](docs/design/视觉约定.md) · [界面设计参考](docs/design/filem-concept.png)
