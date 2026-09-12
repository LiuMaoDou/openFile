# FileM 0.1.1 Windows 路径显示修复

日期：2026-09-12。目标：Windows 11 x64。沿用 local.filem.desktop 应用标识和现有索引格式，升级保留已添加范围。

## 修复内容

Windows 的路径规范化会产生扩展路径语法，例如 \\?\D:\。旧版将内部路径直接用于界面和剪贴板；磁盘根目录的名称也回退为该字符串。“所在目录”还会混用两种目录分隔符。

现在所有相关返回数据统一处理显示格式：

- 范围名称、范围设置：磁盘根目录显示为 D:\。
- 所在目录列、文件详情及完整路径提示：显示为 D:\中文资料。
- 复制路径、删除预览和既有删除记录：保留完整中文文件名，移除常规盘符路径的内部前缀。
- UNC 共享：保留开头两个反斜杠，再显示服务器、共享及目录名称。
- 已有记录在读取时应用同一显示规则，无需重新添加范围或重建索引。离线范围可继续使用界面返回的路径保存设置。

原生路径字节、文件 ID、授权范围和删除复核继续使用原来的内部路径。未知设备名称和 Volume GUID 路径保留原语法。依据：[Rust canonicalize](https://doc.rust-lang.org/std/fs/fn.canonicalize.html)、[Microsoft 路径命名空间](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file)。

## 验证

- macOS 核心测试：27 项全部通过，包含盘符、中文/emoji、长路径、UNC、设备命名空间、原生路径字节保留、重启后查询/删除预览，以及离线范围设置。
- Windows 核心和全部测试目标的 Clippy 严格检查通过；额外的 Windows 专用用例覆盖旧 D 盘索引及 UTF-16 无损往返。Windows 专用测试已编译检查，未在本机执行。
- macOS 全工作区 Clippy、Rust 格式检查、TypeScript 与 Vite 构建通过。
- Windows release 和 NSIS 安装器构建通过；程序为 x64 GUI，无额外 VC++ / WebView2Loader DLL 导入。应用为 13,112,832 字节，安装器为 5,001,625 字节。
- 安装包解包检查通过。安装版与免安装应用仅有预期的 Tauri 包类型标记差异，其余字节一致。

界面流程：加载带 D 盘范围的 Windows 路径样本 → 打开范围设置 → 选择文件 → 核对所在目录列和详情 → 点击复制路径并读取剪贴板 → 打开删除预览；另核对 UNC 共享路径。

使用现有 Playwright 与本机 Chrome 无头模式，在 http://127.0.0.1:5178/、1440×920 和 960×720 视口执行。当前会话未提供 Browser 技能，Playwright 的配套浏览器缺失，故使用已安装 Chrome，无新增浏览器依赖。页面标题、非空界面、无错误覆盖层、控制台、截图及交互检查通过。D 盘和 UNC 是隔离的 API 测试样本，复制按钮已实际点击，剪贴板内容逐字比较；本次界面测试没有操作真实 Windows 磁盘或执行删除。

当前主机仍为 macOS；Windows 原生启动、目录选择、扫描和回收站操作需在 Windows 实机确认。本包仍未签名。构建方式及 WebView2 依赖与 [0.1.0 构建记录](Windows_构建记录.md) 相同。

## 使用

先退出旧版，再安装 FileM_0.1.1_x64-setup.exe，或完整解压 FileM-0.1.1-windows-x64-portable.zip 后双击 FileM.exe。原有范围会自动按新规则显示。
