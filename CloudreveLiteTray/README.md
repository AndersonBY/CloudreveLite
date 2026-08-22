# CloudreveLiteTray — Windows 托盘分享工具

CloudreveLite 的 Windows 桌面客户端：资源管理器右键任意文件 →「上传到 CloudreveLite 并分享」→
直传对象存储 → 自动创建临时分享链接并复制到剪贴板。托盘常驻、开机自启、内存占用极小。

设计对标 `E:\Projects\oss-share`（Tauri 版），但以纯 Rust + Win32 重写：无 WebView、无运行时依赖。

## 关键指标（2026-08-22 构建实测）

| 产物 | 大小 | 说明 |
| --- | --- | --- |
| `CloudreveTray.exe` | 1.01 MB | 托盘主程序，仍为纯 Rust + Win32/GDI |
| `cloudreve_shell.dll` | 221 KB | 资源管理器右键扩展（IExplorerCommand COM） |
| `CloudreveShell.msix` | 7.6 KB | 稀疏包，Win11 新版右键菜单一级入口 |
| `CloudreveLiteTray-Setup.exe` | 1.50 MB | 单文件图形安装包（内嵌以上全部） |

对照：Tauri 参考实现常驻内存 80–150 MB。v0.3.0 release 隔离实测中，安装器待机、
设置窗口和上传窗口的私有内存分别约 2.71 MB、2.96 MB 和 3.00 MB；这些是界面预览进程
快照，正式安装后的长期托盘内存仍应单独复测（v0.2.0 旧快照约 7.6 MB）。

## 架构

```
CloudreveTray.exe（无参数）        托盘模式：Shell_NotifyIcon + 隐藏窗口消息循环
  ├─ pipe 线程   \\.\pipe\cloudreve-lite-tray  收 {"action":"upload"|"quit","files":[...]}
  ├─ worker 线程 上传队列：会话 → 分块 PUT（流式，1MB 级缓冲）→ 完成 → 分享
  └─ 设置对话框   内存 DLGTEMPLATE，服务器/配对码/有效期/下载次数/私密分享
CloudreveTray.exe <文件...>       启动器模式：转发给常驻实例；不在则先拉起再转发
cloudreve_shell.dll               IExplorerCommand::Invoke → 取选中路径 → 命名管道
```

- **认证**：网页端 设置→密码与安全→连接 CLI 生成一次性配对码（`crp_…`），设置对话框里换取
  API Key（`crl_…`），DPAPI 加密存放于 `%APPDATA%\CloudreveLiteTray\config.json`。
- **上传**：`PUT /api/v4/file/upload` 建会话 → 预签名 URL 分块直传 OSS/S3（4 次重试、指数退避）
  → OSS `x-oss-complete-all` / S3 XML complete + 回调 → `PUT /api/v4/share` 取回完整分享链接。
  同名冲突（code 40004）自动以 `entity_type=version` 覆盖重试。
- **反馈**：气泡通知 + 链接进剪贴板（多文件多行），点气泡打开链接；失败为红色错误气泡。
- **右键菜单**：HKCU 注册表经典菜单（Win10/“显示更多选项”）+ 稀疏 MSIX（Win11 新版菜单一级入口），
  全部当前用户级，无需管理员。
- **自启**：`HKCU\...\Run`，安装器写入，托盘菜单可开关。
- **日志**：`%APPDATA%\CloudreveLiteTray\app.log`（每次启动截断，排查用）。

## 构建

前置：Rust（MSVC toolchain）、Windows SDK（rc.exe/makeappx/signtool）。

```powershell
pwsh -NoProfile -File build.ps1            # 全量构建
pwsh -NoProfile -File build.ps1 -FreshCert # 重生成本地签名证书
python assets\gen_icon.py                                               # 重新生成图标/Logo
```

产物在 `dist\CloudreveLiteTray-Setup.exe`。签名用本机自建证书
（CN=CloudreveLiteTray，存于 `build\cert\`，pfx 密码在 `build\cert\pfxpassword.txt`，勿外传）。

正式发布由 `.github/workflows/tray-release.yml` 自动完成。创建并推送与 Cargo 版本一致的
`tray-vX.Y.Z` 注解标签后，GitHub Actions 会在 `windows-latest` 上运行测试、构建和签名，
再把单文件安装包与 `SHA256SUMS.txt` 上传到同名 GitHub Release。发布使用仓库加密 Secrets
`CLOUDREVE_TRAY_SIGNING_PFX_BASE64` 和 `CLOUDREVE_TRAY_SIGNING_PASSWORD` 中的稳定证书；
证书私钥、密码、payload 和本机构建目录均不会进入 Git。

```powershell
pwsh -NoProfile -File scripts\check-version.ps1 -ExpectedVersion 0.3.0
git tag -a tray-v0.3.0 -m "CloudreveLite Tray v0.3.0"
git push origin tray-v0.3.0
```

## 安装 / 卸载（用户视角）

1. 双击 `CloudreveLiteTray-Setup.exe`。安装过程中会弹**一次** UAC（Windows PowerShell）
   用于把签名证书导入机器根存储——这是 Windows 11 现代右键菜单的签名链硬性要求，
   请选「是」；拒绝则只注册经典菜单。重装时不再重复弹窗。
2. 托盘出现图标后右键 → 设置：填服务器地址（默认 `https://share.zaowanwu.com`）+
   网页端配对码 → 保存（配对码一次性，仅在保存时兑换；测试按钮只 ping 服务器/验证已有 Key）。
3. 右键任意文件 →「上传到 CloudreveLite 并分享」；Win11 新菜单直接可见，否则在
   「显示更多选项」里。
4. 卸载：设置→应用→安装的应用→「CloudreveLite 托盘分享工具」，或运行
   `"...\CloudreveLiteTray\CloudreveLiteTray-Setup.exe" --uninstall`。
   卸载保留 `%APPDATA%\CloudreveLiteTray`（配置/日志），可手动删。

## 修复记录

- 2026-08-22 v0.3.0：原生界面整体打磨。
  ① 安装器改为无终端窗口的图形安装向导，提供安装说明、阶段进度、系统确认提示以及
  明确的成功/失败结果；卸载流程同步使用图形界面。
  ② 设置窗口重新组织为“连接”和“默认分享规则”两组，增加密钥显示控制、内联连接状态、
  更清晰的标签和主次按钮，不再依赖连续弹窗反馈。
  ③ 上传窗口改为浅色状态面板，连续进度条同时显示文件序号、文件名、已传输大小、速度和
  剩余时间；完成、失败、取消使用不同状态颜色，长文件名自动收敛。
  ④ 修复 125%–200% 显示缩放下安装器、设置和上传窗口被 Windows 位图拉伸的问题：
  显式启用 Per-Monitor V2，并按窗口 DPI 重算尺寸、字体、图标和圆角。
  全部继续使用原生 Win32/GDI 与系统字体，没有引入 WebView、GPUI 或常驻动画框架。
- 2026-08-22 v0.2.0：上传体验重做。
  ① 新增上传进度窗口（右下角小窗：文件名、进度条、百分比/速度/剩余时间、取消按钮）——
  大文件上传不再像卡死；上传线程通过原子计数汇报字节、窗口线程 250ms 轮询刷新，
  取消即时生效（中断当前分块，取消剩余文件）。完成后显示"分享链接已复制"3 秒自动关闭；
  失败停留等用户关闭。
  ② 修复完成通知看不到的问题：进程设置 AUMID + 安装器创建开始菜单快捷方式，托盘气泡
  在 Win11 上能正常转为系统通知（仍可能受系统"勿扰"影响，进度窗口是可靠反馈）。
  ③ 界面现代化（保持零依赖）：进度窗/设置窗 DWM 主题色标题栏 + 暗色模式适配，提示文字
  灰化。未采用 GPUI：GPU 渲染栈会把常驻内存从 ~7MB 推高到几十 MB 且 API 不稳定，
  与"极小内存"的立项要求冲突。
- 2026-08-22 v0.1.3：修复上传永远卡在"上传中"的两个叠加 bug：
  ① 服务端 `storage_policy.relay` 带 omitempty（直传时字段缺失），客户端按必填解析失败
  → 会话已在服务器创建但客户端报错且无法取消 → 孤儿会话占锁，后续上传报 Lock conflict
  (40073)。修复：所有会话字段宽容解析；解析失败也提取 session_id 主动取消；Lock conflict
  给出"网页端删除上传中条目或等待过期"的明确提示。
  ② 直传 OSS 失败 `native-tls: unable to find any user-specified roots`：ureq 3 默认
  WebPki 根会迫使 native-tls/schannel 手动验证根（禁用系统存储与 AIA 补链），而 OSS 只发
  叶子证书 → 链验证失败。修复：`RootCerts::PlatformVerifier`（系统根 + AIA）。
  另：设置对话框模板控件数声明错误（cdit=10 实为 13）导致"测试连接/保存/取消"三个按钮
  未被创建——改为自动计数；未配置时点上传直接弹出设置窗口。
- 2026-08-22 v0.1.2：修复 Windows 11 现代右键菜单不出现的问题。根因：AppX/MSIX 部署
  服务做签名链校验时只认**机器级** Root 存储，CurrentUser\Root 无效（安装时报
  `0x800B0109 应用包签名的根证书必须是受信任的`）。安装器现在仅在导入证书这一步通过
  UAC 提权（隐藏的 PowerShell 一次性导入 LocalMachine\Root + TrustedPublisher），
  其余步骤仍为用户级；重装时检测到证书已在机器存储则不再弹 UAC。卸载时同样提权移除。
  v0.1.1：修复托盘右键菜单弹出位置错误 —— 弃用 NOTIFYICON_VERSION_4 回调
  坐标解析（版本 4 的参数打包极易误读，且实测菜单锚点跑偏），改为经典 version 0 语义 +
  菜单显示时 `GetCursorPos()` 定位（与 tray-icon crate/oss-share 的成熟做法一致）；
  同时修复气泡点击通知码（NIN_BALLOONUSERCLICK = 0x0405，此前误用 0x0402），
  为 `TrackPopupMenuEx` 补 WM_NULL 消息（KB135788），窗口创建不再对 WS_POPUP 传
  CW_USEDEFAULT。托盘回调现在会写入 app.log（`tray msg: notif=0x…`），便于定位
  菜单/点击类问题。

## 已知边界 / 待办

- 仅支持直传型 OSS/S3 存储策略（Lite 的定位即如此）；中转模式会明确报错。
- 右键菜单只覆盖文件（`*`），不支持文件夹整体上传。
- MSIX 现代菜单要求 Win11（10.0.22000+）；Win10 走经典菜单。
- 依赖服务器站点 URL 配置正确（分享链接由服务端拼装）。

## 目录

```
app\       托盘主程序（main/tray/dlg/config/pipe/upload/api/util）
shell\     IExplorerCommand COM DLL
installer\ 单文件安装器（include_bytes! 内嵌 payload\）
sparse\    稀疏 MSIX 清单
assets\    图标生成器（纯 stdlib Python）与产物
build.ps1  一键全量构建
```
