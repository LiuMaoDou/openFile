<aside>
🎯

核心目标：在不移动磁盘上任何文件的前提下，把**用户指定的盘或文件夹**内相同扩展名的文件聚合到一个视图中浏览与批量操作。

</aside>

## 1. 需求与范围

### 1.1 核心需求

- 用户显式添加一个或多个**监视范围**：整个盘，或任意文件夹（可含子目录）
- 范围可随时增删，每个范围有独立的排除规则与递归开关
- 仅索引范围内的文件，范围之外完全不触碰
- 按扩展名聚合展示，如点击 `.psd` 即列出所有范围内的 PSD 文件
- 支持扩展名大类归组（图片 / 视频 / 文档 / 代码等），同时保留按具体后缀细分
- 磁盘上的文件**位置与内容完全不变**，虚拟视图只持有引用
- 视图内可直接执行真实操作：打开、系统右键菜单、拖拽、重命名、删除、批量移动

### 1.2 非目标（首版不做）

- 替代系统资源管理器的 Shell 角色
- 无配置的全盘自动索引（范围必须由用户显式指定）
- 文件内容全文检索
- 跨设备 / 云端同步

### 1.3 关键约束

| 约束 | 影响 |
| --- | --- |
| 范围可以是文件夹，不一定对齐整个卷 | MFT 枚举只适用于「整盘」范围，不能作为通用方案 |
| 默认路径必须在无管理员权限下可用 | 以目录遍历为主力，MFT 仅作整盘优化 |
| 范围可能嵌套或重叠 | 需要归属判定与去重规则 |
| 范围根可能离线（外置盘 / 网络盘） | 索引保留但标记离线，操作前校验路径 |
| 聚合视图中同名文件极多 | 列表必须默认显示所属目录列 |
| 云盘占位文件不可无感打开 | 需识别并标记，避免触发意外下载 |

## 2. 整体架构

```
范围管理器
  用户指定的盘 / 文件夹 · 排除规则 · 递归开关 · 在线状态
        │
        ├── 范围扫描器
        │     并行目录遍历（默认）｜ MFT 枚举（仅整盘 + 提权）
        │           └──> 索引存储
        │
        └── 变更监听器
              ReadDirectoryChangesW（每范围一个）｜ USN Journal（整盘）
                    └──> 索引存储（增量）

索引存储
  目录表 · FileRecord 表 · 字符串池 · 扩展名倒排索引 · 分范围持久化
        │
        ▼
查询引擎
  扩展名 / 分组命中 · 范围筛选 · 过滤 · 排序 · 分页
        │
        ▼
UI 层
  范围侧栏 │ 扩展名侧栏 │ 虚拟化列表 │ 预览 │ 批量操作面板
        │
        ▼
Shell 交互层
  ShellExecuteEx · IContextMenu · IDataObject
  IFileOperation · IShellItemImageFactory
```

## 3. 数据结构设计

### 3.1 文件记录

路径**不存完整字符串**。目录单独建表，文件只存 `dir_id`，展示时沿目录父链回溯拼接。范围有限时目录数量远小于文件数，这个设计能显著压缩内存。

```rust
struct FileRecord {
    dir_id: u32,        // 指向目录表，回溯得到完整路径
    name_offset: u32,   // 指向字符串池的偏移
    name_len: u16,
    ext_id: u16,        // 扩展名字典 ID
    scope_id: u8,       // 所属监视范围
    attributes: u32,    // FILE_ATTRIBUTE_* 位标志
    size: u64,
    mtime: i64,         // Unix 时间戳
}

struct DirRecord {
    parent_id: u32,     // 父目录；范围根指向自身
    name_offset: u32,
    name_len: u16,
    scope_id: u8,
}
```

单条 `FileRecord` 约 32 字节。1 万文件不到 1 MB，10 万文件约 3 MB，100 万文件约 32 MB。指定范围后绝大多数场景的整体内存占用在 **10–50 MB**，远低于全盘方案。

### 3.2 索引结构

```rust
struct Scope {
    id: u8,
    root: PathBuf,          // 如 "D:\" 或 "D:\Projects\Assets"
    excludes: Vec<String>,  // 相对范围根的排除规则，支持通配
    recursive: bool,
    watch: bool,            // 是否常驻监听变更
    online: bool,           // 外置盘 / 网络盘可能离线
}

struct Index {
    scopes: Vec<Scope>,
    dirs: Vec<DirRecord>,
    records: Vec<FileRecord>,
    name_pool: Vec<u8>,                    // 紧凑 UTF-8 字符串池
    ext_dict: HashMap<String, u16>,        // 扩展名 → ID
    ext_postings: HashMap<u16, Vec<u32>>,  // 扩展名 ID → 记录下标列表（倒排）
    path_lookup: HashMap<(u32, u64), u32>, // (dir_id, name_hash) → 记录下标
    dir_children: HashMap<u32, Vec<u32>>,  // 目录 → 子项，用于子树批量失效
}
```

按扩展名查询即 `ext_dict` → `ext_postings` 两次哈希查表，**O(1)** 命中，与总文件量无关。范围筛选在倒排结果上按 `scope_id` 位掩码过滤，成本可忽略。

### 3.3 扩展名分组配置

```json
{
  "groups": [
    { "name": "图片", "exts": ["jpg","jpeg","png","gif","webp","heic","bmp","tiff"] },
    { "name": "设计源文件", "exts": ["psd","ai","sketch","fig","xd","afdesign"] },
    { "name": "RAW", "exts": ["cr2","cr3","nef","arw","dng","raf","orf"] },
    { "name": "视频", "exts": ["mp4","mkv","mov","avi","webm","flv"] },
    { "name": "文档", "exts": ["pdf","docx","xlsx","pptx","md","txt"] },
    { "name": "压缩包", "exts": ["zip","7z","rar","tar","gz","zst"] },
    { "name": "代码", "exts": ["rs","ts","py","go","c","cpp","java"] }
  ]
}
```

用户可自定义分组。未归类扩展名自动落入「其他」并按数量排序。

## 4. 索引构建

### 4.1 策略选择

按范围类型自动选择扫描方式：

| 范围类型 | 策略 | 预期耗时 |
| --- | --- | --- |
| 单个文件夹（万级文件） | 并行目录遍历 | < 1 秒 |
| 大文件夹（十万级文件） | 并行目录遍历 | 数秒 |
| 整个 NTFS 盘（百万级） | MFT 枚举 + 范围过滤（需提权） | 十几秒 |
| 整个非 NTFS 盘 / 网络盘 | 并行目录遍历 | 数十秒起 |

<aside>
✅

指定范围带来的最大架构收益：**绝大多数场景不再需要管理员权限**。MFT 枚举从架构必需前提降级为「整盘范围」的可选优化，非 NTFS、网络盘、U 盘也不再是特例。

</aside>

### 4.2 并行目录遍历（默认路径）

- `FindFirstFileExW` + `FIND_FIRST_EX_LARGE_FETCH`，单次返回更多条目以减少系统调用
- 工作窃取队列：发现子目录即推入队列，线程池按 CPU 核数并行消费
- 长路径统一使用 `\\?\` 前缀，突破 260 字符限制
- 重解析点（符号链接、Junction）默认不跟随，避免无限递归；可配置开启并做环检测
- 扫描过程中流式推送结果，UI 边扫边出，不等全部完成

### 4.3 整盘范围的 MFT 加速（可选）

仅当范围是整个 NTFS 卷根、且进程已提权时启用。

流程：

1. `CreateFileW("\\\\.\\C:", GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, ...)` 打开卷句柄（需管理员）
2. `DeviceIoControl(FSCTL_QUERY_USN_JOURNAL)` 获取当前 `UsnJournalID` 与 `NextUsn`，**先记录下来**
3. 循环 `DeviceIoControl(FSCTL_ENUM_USN_DATA)`，每次取 64 KB–1 MB 缓冲
4. 逐条解析 `USN_RECORD_V2/V3`，提取 `FileReferenceNumber`、`ParentFileReferenceNumber`、`FileName`、`FileAttributes`
5. 按范围与排除规则过滤后写入 `records`，同步构建目录表与 `ext_postings`

<aside>
⚠️

步骤 2 必须在枚举**开始前**执行。若枚举后才取 USN，扫描期间发生的变更会永久丢失。

</aside>

注意：`FSCTL_ENUM_USN_DATA` 只返回文件名和属性，**不含大小和时间戳**。这两个字段需要在懒加载时用 `GetFileAttributesEx` 按需补齐，或接受首版不显示。

### 4.4 默认排除规则

每个范围可独立覆盖。默认排除以下噪音（相对范围根匹配，支持通配）：

```jsx
Windows\WinSxS, $Recycle.Bin, System Volume Information,
node_modules, .git, AppData\Local\Temp, *.tmp
```

### 4.5 持久化

- 格式：**每个范围一个**二进制快照文件（自定义紧凑格式，避免 SQLite 的写放大）
- 位置：`%LOCALAPPDATA%\<AppName>\scopes\<scope_id>.bin`
- 每个范围记录 `last_scan_time` 与目录 mtime 摘要；整盘 MFT 范围额外记录 `UsnJournalID` + `NextUsn`
- 启动流程：并行加载各范围快照 → UI 立即可用 → 后台按范围校验差异并补齐
- **差异校验**：自顶向下比对目录 mtime，只重扫发生变化的子树，比全量重扫快一到两个数量级
- 范围互相独立：删除范围直接丢弃对应文件；某个范围离线不影响其他范围可用

## 5. 增量更新

### 5.1 主力方案：ReadDirectoryChangesW

范围确定后，这个 API 从「不可靠」变成「首选」：每个范围根目录只需**一个**句柄，开启递归标志即可覆盖整个子树。

实现要点：

- 监听掩码：`FILE_NOTIFY_CHANGE_FILE_NAME | DIR_NAME | SIZE | LAST_WRITE`
- 每范围一个 `OVERLAPPED` 异步请求，用 IOCP 统一收取，线程开销固定与范围数无关
- 缓冲区给足（64 KB 起），并在回调中**立即重新投递**，避免窗口期溢出
- 收到 `ERROR_NOTIFY_ENUM_DIR`（溢出信号）时标记该范围为脏，触发一次子树差异重扫兜底 —— 这是全盘方案做不到的，因为范围有限所以兜底代价可接受
- 事件去抖：同一路径 200 ms 内的多次变更合并处理，避免编辑器保存时的抖动

### 5.2 整盘范围：USN Journal

范围为整盘时监听目录过多，改用卷级 USN Journal：后台线程每 300–1000 ms 调用一次 `FSCTL_READ_USN_JOURNAL`，从持久化的 `NextUsn` 继续读取，可断点续读、重启不丢变更。

关注的原因码：

| 原因码 | 处理动作 |
| --- | --- |
| `USN_REASON_FILE_CREATE` | 插入记录，更新倒排索引 |
| `USN_REASON_FILE_DELETE` | 标记墓碑，从倒排列表移除 |
| `USN_REASON_RENAME_NEW_NAME` | 更新名称与 `dir_id`；扩展名变化则迁移倒排项；移出范围则删除记录 |
| `USN_REASON_DATA_EXTEND` / `OVERWRITE` | 标记大小与 mtime 失效，懒刷新 |
| `USN_REASON_HARD_LINK_CHANGE` | 重新解析父链 |
| `USN_REASON_CLOSE` | 作为事件聚合的提交点 |

### 5.3 删除与碎片整理

删除采用**墓碑标记**而非立即移除，避免 `Vec` 频繁搬移。当墓碑比例超过 20% 时触发一次紧缩（compaction），重建下标映射。

## 6. 查询与展示

### 6.1 列设计（聚合视图必需）

聚合视图中会出现大量同名文件（例如成百个 `IMG_0001.jpg`），**「所在目录」列必须默认开启**，否则视图不可用。

默认列：文件名 · 所在目录（范围内相对路径）· 大小 · 修改时间 · 扩展名 · 所属范围

当只有一个范围时自动隐藏「所属范围」列，避免冗余。

### 6.2 过滤器

```jsx
scope = "D:\Projects\Assets"
size > 10MB
mtime within 30d
path contains "Final"
path not contains "Backup"
duplicates only        // 按 size + 内容哈希
```

### 6.3 虚拟化渲染

列表可能有数十万行，必须使用虚拟滚动，只渲染可见窗口 + 上下缓冲区。缩略图与图标异步加载，快速滚动时取消离屏请求。

### 6.4 排序

排序在下标数组上进行（`Vec<u32>`），不搬动记录本体。百万条排序控制在百毫秒内。

## 7. Shell 交互层

虚拟视图中的每一项都持有可解析的真实路径，所有操作落到真实文件：

| 操作 | 实现 |
| --- | --- |
| 双击打开 | `ShellExecuteExW` |
| 右键菜单 | `IShellFolder::GetUIObjectOf` → `IContextMenu` |
| 拖拽 | `IDataObject`  • `CFSTR_SHELLIDLIST` |
| 复制 / 移动 / 删除 | `IFileOperation`（优于旧的 `SHFileOperation`） |
| 图标 / 缩略图 | `IShellItemImageFactory::GetImage` |
| 属性对话框 | `ShellExecuteEx`  • verb `"properties"` |

<aside>
🔻

第三方右键菜单扩展（Git、7-Zip、网盘）多数只注册到 Explorer。Files 项目同样受此限制。首版接受此差距，通过自建常用命令 + `IFileOperation` 覆盖主要场景。

</aside>

## 8. 风险与应对

| 风险 | 影响 | 应对 |
| --- | --- | --- |
| 用户无管理员权限 | 仅整盘 MFT 加速不可用 | 默认遍历路径本就无需提权；仅在用户添加整盘范围时提示「提权可加速首次扫描」 |
| 云盘占位文件 | 点击触发意外下载、占用带宽 | 检测 `FILE_ATTRIBUTE_RECALL_ON_OPEN` / `RECALL_ON_DATA_ACCESS`，加图标标记并在批量操作前警告 |
| 删除语义混淆 | 用户以为只是移出视图，实际删了文件 | 默认走回收站；首次删除弹强提示；提供「从视图隐藏」作为独立动作 |
| 系统目录噪音 | 数十万无意义条目淹没结果 | 默认排除列表 + 用户可配置；系统文件单独折叠 |
| USN Journal 被禁用或容量过小 | 增量更新失效 | 检测容量，必要时提示启用或扩容；失败则回退定时增量重扫 |
| 索引与磁盘不一致 | 显示已不存在的文件 | 操作前校验路径存在性，失败即时修正索引 |
| 范围嵌套或重叠 | 同一文件重复出现在多个范围 | 添加时检测包含关系并提示合并；重叠部分按最内层范围归属，查询结果按路径去重 |
| 范围根被移动或删除 | 该范围索引整体失效 | 监听范围根的父目录；失效时标记为「不可用」并提供重新定位入口 |
| 外置盘 / 网络盘离线 | 命中已不可访问的路径 | 范围标记离线，结果置灰但保留索引；重新连接后自动差异校验 |
| 内存占用超预期 | 超大范围下体验下降 | 字符串池去重、分范围懒加载、提示用户收窄范围或加排除规则 |

## 9. 技术选型

| 层 | 选择 | 理由 |
| --- | --- | --- |
| 索引内核 | **Rust** | 零成本抽象、内存安全，`windows-rs` 覆盖全部所需 Win32 API |
| UI | **Tauri 2 + 前端框架** 或 **Slint** | Tauri 生态成熟、体积远小于 Electron；Slint 性能更佳但生态较小 |
| 备选 UI | C++ / Win32 + Direct2D | 性能上限最高，但开发效率低 |
| 明确排除 | WinUI 3 / .NET | 启动与滚动性能是 Files 项目最主要的用户抱怨点 |
| 持久化 | 自定义二进制快照 | 避免 SQLite 在百万级写入时的开销 |
| 缩略图 | 系统 `IShellItemImageFactory`  • 自建磁盘缓存 | 复用系统已有缩略图，避免重复解码 |

## 10. 分阶段计划

### 阶段一：可行性验证

- [ ]  并行目录遍历原型，实测十万级文件夹的扫描耗时与内存峰值
- [ ]  `ReadDirectoryChangesW` 递归监听原型，验证缓冲区溢出后的恢复逻辑
- [ ]  目录 mtime 差异校验原型，验证启动补齐的正确性与速度
- [ ]  决策：确认 Rust + Tauri 方案的滚动性能满足要求

### 阶段二：核心可用

- [ ]  范围管理：添加 / 删除盘或文件夹、排除规则、递归开关、在线状态
- [ ]  索引构建 + 分范围持久化 + 启动差异校验
- [ ]  范围侧栏 + 扩展名侧栏 + 虚拟化列表 + 目录列
- [ ]  双击打开 + 系统右键菜单

### 阶段三：批量能力

- [ ]  多选 + 批量移动 / 复制 / 删除（`IFileOperation`，带进度与撤销）
- [ ]  过滤器（大小、时间、路径、所属范围）
- [ ]  扩展名分组配置界面
- [ ]  预览窗格（图片 / PDF / 文本 / 媒体）

### 阶段四：差异化

- [ ]  重复文件检测（size 预筛 → 分块哈希确认）
- [ ]  批量重命名（模板 + 正则 + 预览）
- [ ]  整盘范围的 MFT / USN 加速路径（需提权）
- [ ]  按场景的垂直能力（RAW/EXIF 批处理、Git 状态、素材代理等，择一深入）

## 11. 前置验证建议

在写代码前，用现成工具跑一到两周，确认真实痛点：

1. Everything 输入 `ext:psd path:D:\Projects`，或在目标文件夹内搜索 `ext:psd` 后保存为 `.search-ms`
2. 记录实际使用中的不满：是「找不到」，还是「找到之后不好批量处理」？

<aside>
💡

大概率会发现「按后缀聚合」本身并不稀缺——稀缺的是聚合之后的**批量操作、快速预览与去重**。若验证结果如此，应把阶段三、四的优先级提前，阶段二仅做到够用即可。

</aside>