# 架构与 COM 接口设计

## 1. 目标和边界

工程目标是生成一个便携的 Windows 10/11 x64 桌面程序：

```text
命令行 / .rdp
       │
       ▼
无损解析与覆盖合并
       │
       ▼
SessionConfig（不把明文密码写入 RDP 文本）
       │
       ▼
Win32 主窗口 ── OLE ActiveX 容器 ── mstscax.dll
                                      │
                                      ▼
                                 RDP / RemoteApp
```

程序使用系统注册的 `CLSID_RemoteDesktopClient`
(`EAB16C5D-EED1-4E95-868B-0FBA1B42C092`)。这个类由 `mstscax.dll` 实现，因此
Windows 更新 RDP 控件时程序自动使用系统版本，无需捆绑或注册 OCX。

选择 `IRemoteDesktopClient` 的关键原因是其
`IRemoteDesktopClientSettings::ApplySettings` 接口能接收完整 `.rdp` 内容。相比逐个
调用旧式 `IMsRdpClientAdvancedSettings*` 属性，这种方式不会在 Rust 代码里人为缩小
RDP 属性范围，更适合“保留未知项并尽量让系统识别”的目标。

相关微软文档：

- [RemoteDesktopClient class](https://learn.microsoft.com/en-us/windows/win32/termserv/remotedesktopclient)
- [IRemoteDesktopClientSettings](https://learn.microsoft.com/en-us/windows/win32/api/rdpappcontainerclient/nn-rdpappcontainerclient-iremotedesktopclientsettings)
- [ApplySettings](https://learn.microsoft.com/en-us/previous-versions/hh448591(v=vs.85))
- [Remote Desktop ActiveX interfaces](https://learn.microsoft.com/en-us/windows/win32/termserv/remote-desktop-web-connection-reference)

## 2. Rust 模块

| 模块 | 职责 | 平台 |
|---|---|---|
| `rdp` | 编码识别、逐行解析、未知项保留、属性覆盖、重新编码 | 跨平台 |
| `config` | 默认值、RDP 文件、CLI 和交互输入的合并 | 跨平台 |
| `cli` | GNU 参数及 `mstsc.exe` 斜杠参数兼容 | 跨平台 |
| `windows::ui` | Win32 窗口、补全表单、消息循环、生命周期 | Windows |
| `windows::activex` | OLE 容器、RDP 控件、设置、事件、密码保护 | Windows |

公开库接口位于 `mstsc_rs`：

```rust
use mstsc_rs::{ConnectionOverrides, SessionConfig};

let overrides = ConnectionOverrides {
    server: Some("rdp.example.com:3389".into()),
    username: Some(r"CONTOSO\alice".into()),
    width: Some(1920),
    height: Some(1080),
    ..Default::default()
};

let config = SessionConfig::resolve(Some("base.rdp".into()), overrides)?;

#[cfg(windows)]
mstsc_rs::windows::run(config)?;
```

`SessionConfig::rdp_settings_text()` 只返回非明文密码的设置流，可用于审计和 dry-run。

## 3. ActiveX 承载

ActiveX 不是普通子窗口。宿主实现以下 OLE 接口：

- `IOleClientSite`
- `IOleWindow`
- `IOleInPlaceSite`
- `IOleInPlaceUIWindow`
- `IOleInPlaceFrame`

创建顺序：

1. 将线程初始化为 STA，并初始化 OLE；
2. `CoCreateInstance(CLSID_RemoteDesktopClient, CLSCTX_INPROC_SERVER)`；
3. 查询 `IOleObject` 和 `IRemoteDesktopClient`；
4. `IOleObject::SetClientSite`；
5. `OleSetContainedObject`；
6. `IOleObject::DoVerb(OLEIVERB_INPLACEACTIVATE)`；
7. 取得 `IRemoteDesktopClientSettings` 并 `ApplySettings`；
8. 附加事件回调；
9. `IRemoteDesktopClient::Connect`。

窗口尺寸变化时调用 `IOleInPlaceObject::SetObjectRects`。启用
`dynamic resolution:i:1` 后，同时调用
`IRemoteDesktopClient::UpdateSessionDisplaySettings`。

消息循环先把键盘消息交给 `IOleInPlaceActiveObject::TranslateAccelerator`，确保
远程桌面的组合键和 ActiveX 内部导航正常工作。

关闭顺序：

1. 解除事件；
2. `Disconnect`；
3. UI/In-place deactivate；
4. `IOleObject::Close(OLECLOSE_NOSAVE)`；
5. 清空 client site；
6. 释放 COM 对象并反初始化 OLE/WinRT。

## 4. 事件

程序订阅：

- `OnConnecting`
- `OnConnected`
- `OnLoginCompleted`
- `OnDisconnected`
- `OnDialogDisplaying`
- `OnDialogDismissed`
- `OnNetworkStatusChanged`
- `OnRemoteDesktopSizeChanged`
- `OnStatusChanged`

回调对象实现 `IDispatch`，把事件投递回主窗口线程。连接状态用于窗口标题和日志；
断开参数用于诊断。证书、凭据和重定向警告对话框由系统控件显示，宿主不替换或自动
确认它们。

## 5. 凭据

密码来源按优先级：

1. `--password`
2. `--password-env NAME`
3. 默认环境变量 `MSTSC_RS_PASSWORD`
4. Win32 补全界面

明文密码不会加入普通 `.rdp` 文本。连接前使用 Windows
`DataProtectionProvider("LOCAL=user")` 保护，并按微软要求设置：

```text
WinRTPasswordEncoding:i:1
WinRTEncryptedPassword:s:<base64>
```

`1` 表示 UTF-16LE。保护后的数据只对当前 Windows 用户有效。控件复制设置后，程序
立即销毁密码编辑框并清除 `SecretString`；但命令行密码仍可能被进程查看工具或 shell
历史捕获，因此环境变量方式更合适。

微软接口要求见
[SetRdpProperty](https://learn.microsoft.com/en-us/windows/win32/api/rdpappcontainerclient/nf-rdpappcontainerclient-iremotedesktopclientsettings-setrdpproperty)。

## 6. 安全对话框

实现不提供 `--ignore-certificate-errors`，不修改系统信任库，也不自动点击安全提示。
证书不匹配、自签名证书、身份验证和资源重定向警告继续由 Windows 控件及组策略决定。

2026 年 4 月后的 Windows RDP 控件默认使用新版连接安全对话框。本项目不设置
`RedirectionWarningDialogVersion`，因此遵循系统默认和管理员策略。微软说明：

- [Understanding security warnings when opening RDP files](https://learn.microsoft.com/en-us/windows-server/remote/remote-desktop-services/remotepc/understanding-security-warnings)
- [IMsRdpExtendedSettings::Property](https://learn.microsoft.com/en-us/windows/win32/termserv/imsrdpextendedsettings-property)

如果原 `.rdp` 带数字签名，任何命令行覆盖都会改变已签名内容，使原签名无法再证明
合并后的配置。程序不会伪造或静默信任该签名，最终提示仍由系统控件决定。

## 7. 线程模型

主窗口、ActiveX、事件回调都在同一个 STA UI 线程。COM 对象不会跨线程发送；
`ActiveXHost` 也不公开 `Send`/`Sync`。这种模型与 ActiveX/OLE 的 in-place activation
要求一致，并避免事件回调和窗口状态之间的锁竞争。
