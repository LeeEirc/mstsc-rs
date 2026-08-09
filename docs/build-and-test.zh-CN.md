# 构建、测试和发布

## 工具链

- Rust `1.97.1`（由 `rust-toolchain.toml` 固定）
- Windows x64 MSVC 本机工具链
- Windows 10/11 x64 运行环境

选择 MSVC ABI 是因为系统 ActiveX/COM、Windows SDK 导入库和部署环境都以官方 MSVC
ABI 为基准。项目只在 Windows 本机和 GitHub Actions Windows runner 构建和运行，
不提供其他宿主平台的构建路径。运行时依赖 Windows 自带的系统 DLL 和已注册的
`mstscax.dll`。

## Windows 原生构建

在 Visual Studio Build Tools 已安装 C++/Windows SDK 的 PowerShell 中：

```powershell
rustup show
cargo test --all-targets
cargo build --release
```

## 测试层次

### Windows 自动测试

- `.rdp` UTF-8/UTF-16 解析；
- 未知字段和重复项保留；
- 字符串值中的冒号；
- CLI 斜杠参数；
- 配置覆盖顺序；
- 缺失字段识别；
- Windows x64 release 构建和 EXE 冒烟测试；
- 从 `System32` 加载 `mstscax.dll`，创建桌面 COM 类、执行 OLE 原位激活，并验证核心
  设置和非脚本凭据接口。

自动测试不建立真实 RDP 网络连接。

### Windows 手工/集成测试

至少准备以下环境：

1. 使用 NLA 的普通 Windows/Windows Server 主机；
2. 自签名或名称不匹配证书，用于确认安全提示可见；
3. RD Gateway；
4. RemoteApp 集合；
5. 双显示器；
6. 可重定向的打印机、磁盘、智能卡和麦克风。

建议验证：

- 直接参数和 UTF-16 `.rdp` 均能连接；
- CLI 值覆盖文件；
- 密码错误时显示系统凭据界面；
- 自签名证书不会被静默接受；
- 各类重定向在安全对话框中可见；
- 动态调整窗口后远端分辨率更新；
- `Ctrl+Alt+Break` 等组合键；
- 断网后的系统自动重连和断开事件；
- RemoteApp 窗口显示与退出。

## 发布

每次推送与 `Cargo.toml` 中版本一致的 `vX.Y.Z` 标签时，GitHub Actions 会在所有检查
通过后自动创建 GitHub Release。也可以从 Actions 页面的 `CI and Release` 工作流
手动填写 `release_tag`；留空时只执行 CI，不创建 Release。

例如发布 `0.1.0`：

```bash
git tag -a v0.1.0 -m "Release v0.1.0"
git push origin v0.1.0
```

Release 提供：

```text
mstsc-rs.exe
mstsc-rs-vX.Y.Z-windows-x64.zip
SHA256SUMS.txt
```

ZIP 中包含 EXE、README 和 MIT 许可证。程序不需要复制 `mstscax.dll`，也不能私自
分发系统 DLL。运行时若系统控件未注册，程序会显示包含创建
桌面 Remote Desktop ActiveX 控件失败上下文和 HRESULT 的错误。

工作流会核对发布标签与 Cargo 包版本，并全部在 Windows runner 上执行格式、Clippy、
单元测试、COM 激活测试、EXE 冒烟测试和 SHA-256 生成。普通 CI 构建的 Windows 包也会作为
Actions artifact 保留 14 天，便于在正式发布前下载测试。

正式分发建议额外执行：

- 对 EXE 做 Authenticode 签名；
- 生成 SHA-256；
- 在干净 Windows 10 和 Windows 11 虚拟机测试；
- 用组织批准的签名流程签署需要分发的 `.rdp` 文件；
- 记录构建使用的 Rust、Cargo.lock 和提交版本。
