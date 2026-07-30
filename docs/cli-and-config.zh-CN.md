# 命令行与配置合并

## 合并优先级

从低到高：

1. 无文件启动时的安全默认值；
2. `.rdp` 文件；
3. 具名 CLI 参数；
4. 按出现顺序处理的 `--set name:type:value`；
5. 缺失字段补全界面。

因此：

```powershell
mstsc-rs.exe base.rdp /f --width 1920 --set "redirectclipboard:i:0"
```

会保留 `base.rdp` 的其他行，把 `screen mode id` 覆盖为 `2`、`desktopwidth` 覆盖为
`1920`，最后关闭剪贴板重定向。

密码单独处理，不会由 `--dry-run` 输出。

## 兼容的 mstsc 参数

| mstsc 形式 | 等价参数 | RDP 属性 |
|---|---|---|
| `/v:host[:port]` | `--server` | `full address:s:` |
| `/f` | `--fullscreen` | `screen mode id:i:2` |
| `/multimon` | `--multimon` | `use multimon:i:1` |
| `/span` | `--span` | `span monitors:i:1` |
| `/w:n` | `--width n` | `desktopwidth:i:` |
| `/h:n` | `--height n` | `desktopheight:i:` |
| `/admin`、`/console` | `--admin` | `administrative session:i:1` |
| `/public` | `--public` | `public mode:i:1` |
| `/restrictedAdmin` | `--restricted-admin` | `restricted admin mode:i:1` |
| `/remoteGuard` | `--remote-guard` | `remote credential guard:i:1` |
| `/?` | `--help` | — |

不支持 `/edit`，因为需求明确不提供保存或另存 `.rdp`。

## 扩展参数

运行 `mstsc-rs.exe --help` 获取完整列表。主要扩展包括：

- `--username`、`--domain`
- `--password`、`--password-env [NAME]`
- `--gateway`
- `--dynamic-resolution`
- `--remote-app`、`--remote-app-args`、`--remote-app-workdir`
- `--redirect-*`
- `--audio-mode`
- `--set`
- `--dry-run`

布尔重定向参数显式接收 `true` 或 `false`：

```powershell
--redirect-clipboard false
```

## `.rdp` 解析规则

属性格式：

```text
name:type:value
```

支持标准类型：

- `i`：32 位整数
- `s`：字符串
- `b`：二进制文本

第三段可以继续包含冒号，例如：

```text
full address:s:server.example.com:3390
```

未识别类型不会报错：

```text
vendor-property:z:any:value
```

只要未被覆盖，这一行会原样保留。对于重复属性，读取时最后一个值生效；覆盖时也修改
最后一个匹配项，之前的重复项保持不动。

## 编码

读取：

- UTF-8
- UTF-8 BOM
- UTF-16 LE BOM
- UTF-16 BE BOM
- 可可靠识别的无 BOM UTF-16

`RdpDocument::to_bytes()` 会按读取时的编码输出。当前 UI 不提供保存功能，但库接口可以
用于调用方自己的存储流程。
