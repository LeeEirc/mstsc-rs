# RDP 属性与功能覆盖

## 全量设置策略

项目不会建立一份“允许列表”来过滤 `.rdp`。解析、合并后调用
`IRemoteDesktopClientSettings::ApplySettings` 传入完整内容：

```text
原始已知属性 ─┐
原始未知属性 ─┼─> 完整 BSTR 设置流 ─> Windows RDP 控件
CLI 新属性   ─┘
```

因此，Windows 当前版本支持而本项目没有具名 CLI 开关的属性，仍可从 `.rdp` 或
`--set` 传入。未知属性会保留，但是否生效由系统控件决定。

微软的属性总表：
[Supported RDP properties](https://learn.microsoft.com/en-us/azure/virtual-desktop/rdp-properties)。

## 具名功能

| 能力 | `.rdp` 属性或接口 |
|---|---|
| 窗口/全屏 | `screen mode id` |
| 尺寸 | `desktopwidth`、`desktopheight` |
| 动态分辨率 | `dynamic resolution` + `UpdateSessionDisplaySettings` |
| 多显示器 | `use multimon`、`selectedmonitors` |
| 跨屏 | `span monitors` |
| 剪贴板 | `redirectclipboard` |
| 打印机 | `redirectprinters` |
| 磁盘 | `drivestoredirect` |
| 动态设备 | `devicestoredirect` |
| USB | `usbdevicestoredirect` |
| 智能卡 | `redirectsmartcards` |
| COM 端口 | `redirectcomports` |
| WebAuthn | `redirectwebauthn` |
| 位置 | `redirectlocation` |
| 摄像头 | `camerastoredirect` |
| 麦克风 | `audiocapturemode` |
| 音频播放 | `audiomode` |
| RD Gateway | `gatewayhostname`、`gatewayusagemethod` 等 |
| NLA/CredSSP | `enablecredsspsupport`、`authentication level` |
| 管理会话 | `administrative session` |
| Restricted Admin | `restricted admin mode` |
| Remote Credential Guard | `remote credential guard` |
| RemoteApp | `remoteapplicationmode`、`remoteapplicationprogram`、`remoteapplicationcmdline` |

## 默认值

没有 `.rdp` 文件时使用：

```text
screen mode id:i:1
session bpp:i:32
compression:i:1
networkautodetect:i:1
bandwidthautodetect:i:1
enablecredsspsupport:i:1
authentication level:i:2
prompt for credentials:i:0
redirectclipboard:i:1
redirectprinters:i:1
redirectsmartcards:i:1
audiomode:i:0
audiocapturemode:i:1
```

磁盘、USB、摄像头和任意动态设备不会在无配置时默认全部开放，需要 `.rdp` 或命令行
显式开启。安全对话框仍会按 Windows 当前策略显示请求的本地资源。

## RemoteApp

可以直接读取标准 RemoteApp `.rdp`。具名参数会设置：

```text
remoteapplicationmode:i:1
remoteapplicationprogram:s:<program>
remoteapplicationcmdline:s:<arguments>
shell working directory:s:<directory>
```

不同 RDS 部署可能要求 `||alias`、完整路径、签名 RDP、workspace ID 或 load balance
信息；这些额外字段由原文件或 `--set` 保留并传入。

## 版本与策略

属性生效需要系统控件和服务器双方支持。例如新的 USB、安全协议或文本处理属性可能只
存在于特定 Windows 11 版本；多显示器、摄像头或智能卡也可能被客户端/服务器组策略
禁止。本项目不绕过组策略。
