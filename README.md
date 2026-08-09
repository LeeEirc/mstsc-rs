# mstsc-rs

`mstsc-rs.exe` 是一个 Rust 编写的 Windows 原生远程桌面客户端。程序不实现自己的
RDP 协议栈，也不启动 `mstsc.exe`；它创建 Win32 窗口并在窗口内承载 Windows 自带
`mstscax.dll` 中的 Remote Desktop ActiveX/COM 控件。

目标平台是 Windows 10/11 x64。生成的 EXE 是便携文件，无需安装，也不会注册自己的
COM 组件。

## 已实现

- 读取 UTF-8、UTF-8 BOM、UTF-16 LE/BE `.rdp` 文件；
- 保留未知属性、未知类型、注释、空行、重复项和未修改行；
- 将合并后的常用 RDP 设置映射到 Windows 桌面 ActiveX 控件，覆盖显示、打印机、
  剪贴板、智能卡、麦克风、音频、Gateway 和 RemoteApp；
- 兼容 `/v:`、`/f`、`/w:`、`/h:`、`/admin` 和 `/public`；其他已解析但尚未映射的
  `mstsc.exe` 开关会保留在 dry-run 设置文本中；
- GNU 风格参数覆盖 `.rdp`，并可用 `--set name:type:value` 设置任意 RDP 属性；
- 支持 `--password`、`--password-env` 和默认的 `MSTSC_RS_PASSWORD`，并通过非脚本
  COM 凭据接口传给系统控件；
- 参数缺少服务器、用户名或密码时显示 Win32 原生补全界面；
- 保留系统证书、身份和本地资源重定向安全提示，不提供静默忽略证书的选项；
- 一个进程、一个窗口、一个 RDP 会话；
- Windows x64 原生构建，并通过 GitHub Actions 的 Windows runner 测试和打包。

## 快速使用

```powershell
mstsc-rs.exe office.rdp
mstsc-rs.exe office.rdp /f /multimon /v:rdp.example.com:3390
mstsc-rs.exe --server rdp.example.com --username CONTOSO\alice --password-env
mstsc-rs.exe config.rdp --width 1920 --height 1080 --password-env RDP_PASSWORD
```

RemoteApp：

```powershell
mstsc-rs.exe remoteapp.rdp `
  --remote-app "||Calculator" `
  --remote-app-args "" `
  --remote-app-workdir "C:\"
```

设备重定向：

```powershell
mstsc-rs.exe host.rdp `
  --redirect-clipboard true `
  --redirect-printers true `
  --redirect-drives "*" `
  --redirect-smartcards true `
  --redirect-cameras "*" `
  --redirect-microphone true
```

属性覆盖（运行时是否生效取决于桌面 ActiveX 接口是否映射该属性）：

```powershell
mstsc-rs.exe host.rdp `
  --set "selectedmonitors:s:0,1" `
  --set "usbdevicestoredirect:s:*" `
  --set "devicestoredirect:s:*"
```

只检查合并结果而不连接：

```powershell
mstsc-rs.exe host.rdp /f --set "redirectclipboard:i:0" --dry-run
```

`--help` 或 `/?` 可查看所有参数。

> `--password` 会让密码出现在进程命令行和可能的 shell 历史中。自动化场景优先使用
> `--password-env NAME`；不传名称时读取 `MSTSC_RS_PASSWORD`。

## 构建

在 Windows PowerShell 中使用本机 MSVC 工具链构建：

```powershell
cargo build --release
```

产物位于：

```text
target/release/mstsc-rs.exe
```

项目只支持在 Windows 上本机构建和运行。ActiveX 控件的加载和原位激活会在 GitHub
Actions 的 Windows runner 上实际执行；证书对话框、设备重定向和真实 RDP 连接仍需在
Windows 10/11 环境中手工集成测试。

## 下载与发布

可直接从 GitHub 仓库的
[Releases](https://github.com/LeeEirc/mstsc-rs/releases) 页面下载：

- `mstsc-rs.exe`：Windows 10/11 x64 便携可执行文件；
- `mstsc-rs-vX.Y.Z-windows-x64.zip`：包含 EXE、README 和许可证的压缩包；
- `SHA256SUMS.txt`：发布文件的 SHA-256 校验值。

推送与 `Cargo.toml` 版本一致的 `vX.Y.Z` 标签后，GitHub Actions 会在 Windows runner
完成格式、Clippy、单元测试、COM 激活测试和发布构建，再自动创建 Release。也可以在 Actions 页面的
`CI and Release` 工作流中手动填写 `release_tag` 发布；留空只运行 CI。

## 文档

- [架构与 COM 接口设计](docs/architecture.zh-CN.md)
- [命令行与配置合并](docs/cli-and-config.zh-CN.md)
- [RDP 属性与功能覆盖](docs/rdp-properties.zh-CN.md)
- [构建、测试和发布](docs/build-and-test.zh-CN.md)

## 能力边界

程序不重新实现微软私有 RDP 协议栈。最终可用能力受客户端 Windows 版本、远端服务器
版本、组策略、RD Gateway 策略和设备驱动限制。解析器会保留未知或尚未映射的属性，
`--dry-run` 也会显示它们，但桌面 ActiveX 控件不会自动应用尚未映射的字段。
