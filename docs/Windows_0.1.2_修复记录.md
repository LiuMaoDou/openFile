# FileM 0.1.2 修复与验证记录

日期：2026-09-13。目标：Windows 11 x64。保留应用标识 `local.filem.desktop`、已添加文件夹和既有索引，不需要删除旧索引。

## 改动

- **多选回收**：Windows Shell 操作改用无损 UTF-16 普通盘符 / UNC 路径，并复核 Shell 解析出的文件身份及实际路径。回收标志包含 `FOF_ALLOWUNDO`；逐项进度回调记录实际 HRESULT 和回收站目标，拦截未标记为可回收的删除，不把“操作已入队”当成成功。失败显示具体原因，保留逐项记录。
- **删除窗口**：提交后显示“等待处理 / 处理中”，不会被尚未开始执行的旧查询退回确认状态；迟到的轮询不能覆盖最终结果。每份计划仍只执行一次，过期、身份变化、失败、中断保留原来的检查和处理。
- **扫描与删除**：回收批次暂停扫描。扫描先结束或丢弃当前读取批次，再允许删除；恢复时重新读取中断的目录，防止将删除前的元数据重新写回索引。删除完成、停止或返回错误时释放暂停。其他文件的读取不等待整批删除结束。
- **打开 / 定位**：放到每行“类型”右侧。Windows 使用 Shell 项目标识和 `SHOpenFolderAndSelectItems` 精确定位，移除 `explorer.exe /select,...` 命令行拼接，解析失败时返回错误，不回退打开文档目录。系统打开只等待分发，不等待应用退出或进入空闲。
- **打开性能**：只查询目标文件的授权文件夹，移除每次打开时对所有文件夹的文件数统计；文件系统身份与路径检查期间释放数据库锁。前端不因打开 / 定位重新查询整个列表，并防止重复点击同一文件重复提交。
- **文案与图标**：统一使用“添加文件夹”“监控文件夹”“文件夹设置”等用语。内置 Material Icon Theme 5.38.1 的 1,377 条后缀关联、369 个 SVG，支持 Office、PDF、Adobe、代码、压缩包、影音等图标；无已知图标的扩展名显示通用图标和后缀。相关后缀可共享同一应用 / 格式图标，不虚构未知格式的品牌标志。图标离线可用，独立资源避免增加主脚本内嵌图片体积。

## 验证环境和交互

Browser 插件技能未提供，按 frontend-testing-debugging 技能使用现有 Playwright 与系统 Chrome 无头模式；未安装新的浏览器。页面为 `http://127.0.0.1:5178/`，覆盖 1600×1000、1440×920、960×720。Windows 路径交互使用隔离的 API 样本；实际回收操作使用本机 macOS 的隔离测试文件。

| 检查 | 结果和证据 |
| --- | --- |
| 页面身份、非空内容 | FileM 标题、文件列表、文件夹与类型导航正常 |
| 框架覆盖层、控制台 | 无 Vite 错误覆盖层，无控制台错误；发现并修复 favicon 404 |
| 图标和行尾操作 | 11 种代表性图标全部成功加载，未知后缀有标识；第 7 列是打开 / 定位，位于类型之后 |
| 鼠标和键盘操作 | 点击 / 双击与 Enter 操作发送正确文件 ID；重复点击只提交一次；操作按钮不会改变多选状态 |
| 打开性能回归 | 打开请求不触发额外列表查询；文件核对不再读取全文件夹统计 |
| 删除窗口竞态 | 模拟执行排队、旧 planned 查询、迟到 running 查询，界面保持单次执行，最终成功 / 失败结果不回退 |
| 列表重排 | 扫描结果重排后打开仍使用所点文件 ID；D 盘复制路径逐字一致 |
| 实际批量回收 | 通过界面选择两个新建测试文件并确认；在系统废纸篓逐一核对文件名和全部内容，随后恢复这两个测试文件 |
| 并发扫描 | 回收同时提交刷新，删除项不留在查询中；未删除文件 ID 与正文保持正确 |
| Rust 核心 | 30 项测试通过，包含扫描暂停/排空/恢复、删除期间刷新、其余文件读取、回收失败、停止、身份变化、单次提交及路径回归 |
| 静态检查 | TypeScript / Vite 构建、macOS 工作区 Clippy、Windows 核心与全部测试目标 Clippy、Rust 格式检查通过 |

截图及交互日志保存在本机 `/tmp/filem-012-qa/`：

- `icons-row-actions.png`、`narrow-row-actions.png`：图标、行尾操作和窄窗口横向滚动。
- `delete-partial-result.png`：模拟部分成功 / 部分失败的最终状态。
- `real-recycle-result.png`：macOS 真实回收的逐项结果。
- `ui-result.json`、`real-result.json`：运行环境与交互断言。

## 分发与限制

退出旧版后安装 `FileM_0.1.2_x64-setup.exe`，或完整解压 `FileM-0.1.2-windows-x64-portable.zip` 并运行 `FileM.exe`。安装版在需要时联网安装 WebView2；免安装版使用系统已有运行时。文件夹和操作记录保存在原来的用户应用数据目录。

Windows release 与 NSIS 构建、压缩包完整性、x64 GUI 架构、静态 CRT 导入和安装器内容已检查。安装器与免安装主程序仅有预期的 Tauri 包类型标记差异。校验值见 `releases/SHA256SUMS.txt`。

**本机没有 Windows 运行环境。** Windows 专用实现已编译检查，Windows 原生回收站、系统打开、定位及大盘启动性能尚未实机验证；macOS 实际回收和浏览器样本不能替代此验证。安装包仍未签名。

## 来源

- [Material Icon Theme 图标源码与 MIT 许可](https://github.com/material-extensions/vscode-material-icon-theme)，使用 npm 发布版本 5.38.1；许可与来源记录随安装包 / 免安装包分发。
- [IFileOperation 操作标志](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-setoperationflags)。
- [PreDeleteItem 回调](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperationprogresssink-predeleteitem)、[PostDeleteItem 实际回收结果](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperationprogresssink-postdeleteitem)。
- [SHOpenFolderAndSelectItems 精确选择文件](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shopenfolderandselectitems)。
- [ShellExecuteEx 标志及等待行为](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-shellexecuteinfow)。
