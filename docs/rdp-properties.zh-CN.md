# RDP 属性与功能覆盖

## 解析与设置映射

项目不会过滤或删除 `.rdp` 中的未知项，解析、合并和 `--dry-run` 都会保留它们。
实际连接使用 Windows 桌面 ActiveX 控件，当前映射如下：

```text
原始/CLI 属性 ─> RdpDocument ─> 已映射属性 ─> 桌面 RDP ActiveX 控件
                                  │
                                  └─ 未映射属性仅保留并用于 dry-run
```

`--set` 可以覆盖任意文本属性，但只有下表中已映射的属性会用于当前连接。

微软的属性总表：
[Supported RDP properties](https://learn.microsoft.com/en-us/azure/virtual-desktop/rdp-properties)。

## 具名功能

| 能力 | `.rdp` 属性或接口 |
|---|---|
| 窗口/全屏 | `screen mode id` |
| 尺寸 | `desktopwidth`、`desktopheight` |
| 自适应窗口 | `dynamic resolution` + `UpdateSessionDisplaySettings`，不支持时回退 `SmartSizing` |
| 剪贴板 | `redirectclipboard` |
| 打印机 | `redirectprinters` |
| 磁盘 | `drivestoredirect` |
| 智能卡 | `redirectsmartcards` |
| COM 端口 | `redirectcomports` |
| 麦克风 | `audiocapturemode` |
| 音频播放 | `audiomode` |
| RD Gateway | `gatewayhostname`、`gatewayusagemethod` 等 |
| NLA/CredSSP | `enablecredsspsupport`、`authentication level` |
| 管理会话 | `administrative session` |
| RemoteApp | `remoteapplicationmode`、`remoteapplicationprogram`、`remoteapplicationcmdline` |

## 默认值

没有 `.rdp` 文件时使用：

```text
screen mode id:i:1
dynamic resolution:i:1
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

窗口化会话默认跟随客户区尺寸调整远端分辨率，并沿用当前显示器 DPI；服务器不支持动态
分辨率时会自动缩放到窗口内。`.rdp` 中显式设置 `dynamic resolution:i:0` 可关闭此行为。

磁盘不会在无配置时默认开放。USB、摄像头和任意动态设备属性目前会被解析器保留，
但尚未映射到桌面控件接口。安全对话框仍会按 Windows 当前策略显示已请求的本地资源。

## RemoteApp

可以直接读取标准 RemoteApp `.rdp`。具名参数会设置：

```text
remoteapplicationmode:i:1
remoteapplicationprogram:s:<program>
remoteapplicationcmdline:s:<arguments>
shell working directory:s:<directory>
```

不同 RDS 部署可能要求 `||alias`、完整路径、签名 RDP、workspace ID 或 load balance
信息；这些额外字段会由原文件或 `--set` 保留，但未映射字段不会自动应用到当前连接。

## 版本与策略

属性生效需要本项目已映射、系统控件支持并且服务器策略允许。例如智能卡可能被客户端
或服务器组策略禁止。本项目不绕过组策略。
