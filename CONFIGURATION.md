# po0-auto 配置参考

本文档说明 `/etc/po0-auto/config.yaml` 的全部配置项，适用于 `po0-auto 0.1.0`。

## 1. 修改配置的标准流程

配置只在进程启动时读取，修改文件后必须 reload 或 restart 才会生效：

```sh
cp /etc/po0-auto/config.yaml /etc/po0-auto/config.yaml.backup
vi /etc/po0-auto/config.yaml
chmod 0600 /etc/po0-auto/config.yaml
/usr/bin/po0-auto --config /etc/po0-auto/config.yaml --check-config
/etc/init.d/po0-auto reload
logread -e po0-auto
```

`--check-config` 只检查 YAML 和字段约束，不请求公网接口、不更新白名单，也不发送 Telegram 通知。

维护时注意：

- YAML 使用空格缩进，不能使用 Tab。
- Token、Chat ID 和包含特殊字符的字符串建议使用双引号。
- 配置不支持 `${ENV_VAR}` 环境变量替换。
- 当前版本可能忽略未知的 YAML 字段，因此字段名应严格参照本文档；校验通过不代表拼错的自定义字段一定会被识别。
- `token` 和 `bot_token` 是敏感信息，不要发送到聊天、提交到 Git 或复制进日志。

## 2. 推荐的完整配置

```yaml
ip_sources:
  - name: bilibili
    url: https://api.bilibili.com/x/web-interface/zone
    response: bilibili_json
    timeout_seconds: 8
  - name: ipinfo
    url: https://ipinfo.io/ip
    response: plain_text
    timeout_seconds: 8
  - name: ident
    url: https://v4.ident.me
    response: plain_text
    timeout_seconds: 8
  - name: aws-checkip
    url: https://checkip.amazonaws.com
    response: plain_text
    timeout_seconds: 8
  - name: ifconfig
    url: https://ifconfig.me/ip
    response: plain_text
    timeout_seconds: 8

firewall:
  base_url: https://console.po0.io/modules/servers/penguin/api/firewall.php
  timeout_seconds: 15

servers:
  - name: primary
    token: "REPLACE_WITH_SERVER_TOKEN"

telegram:
  enabled: false
  api_base: https://api.telegram.org
  bot_token: "REPLACE_WITH_NEW_BOT_TOKEN"
  chat_id: "REPLACE_WITH_CHAT_ID"
  timeout_seconds: 15

detection:
  fast_interval_seconds: 300
  slow_interval_seconds: 1800
  slow_until_percent: 75
  history_sample_size: 10
  retention_days: 7
  retry_initial_seconds: 30
  retry_max_seconds: 900

storage:
  state_file: /etc/po0-auto/data/state.json
  history_file: /etc/po0-auto/data/history.json

user_agent: po0-auto/0.1.0
```

## 3. `ip_sources`：公网 IPv4 查询源

`ip_sources` 是非空列表。程序严格按照配置顺序请求；当前接口超时、网络失败、返回非 2xx、响应格式错误或不是公网 IPv4 时，自动尝试下一项。所有接口均失败时，本轮不改变当前 IP，也不会更新白名单。

每个查询源包含：

| 字段 | 类型 | 默认值 | 说明 |
|---|---:|---:|---|
| `name` | 字符串 | 无 | 日志中使用的名称，不能为空且必须唯一。 |
| `url` | 字符串 | 无 | 查询地址，必须是带主机名的 HTTP 或 HTTPS URL。 |
| `response` | 枚举 | `plain_text` | 响应解析方式，只能是 `plain_text` 或 `bilibili_json`。 |
| `timeout_seconds` | 正整数 | `8` | 单个查询源的超时时间，单位秒。 |

响应类型：

- `plain_text`：去除首尾空白后，整个响应必须是一个合法公网 IPv4，例如 `1.1.1.1`。
- `bilibili_json`：要求业务码 `code` 为 `0`，并从 `data.addr` 读取 IPv4。

程序会拒绝 IPv6、私网地址、回环地址、链路本地地址、CGNAT 地址、文档保留地址和多播地址。

调整建议：

- 保留至少两个不同运营方的接口，避免单点故障。
- 频繁超时的接口可以后移或移除。
- 不建议把不可信的第三方接口放在首位，因为其返回值会决定是否更新白名单。

如果整个 `ip_sources` 配置块被省略，程序使用示例中的五个默认接口和顺序。

## 4. `firewall`：白名单 API

| 字段 | 类型 | 默认值 | 说明 |
|---|---:|---:|---|
| `base_url` | 字符串 | po0.io 官方地址 | 白名单 API 基础地址，必须为 HTTP 或 HTTPS URL。 |
| `timeout_seconds` | 正整数 | `15` | 每台服务器单次更新请求的超时时间，单位秒。 |

程序会自动在 `base_url` 后附加：

```text
action=add&token=对应服务器Token
```

因此 `base_url` 建议只填写到 `firewall.php`，不要手工加入 `action` 或 `token`。HTTP 2xx 被视为成功，其他状态码或网络错误会进入独立重试。

## 5. `servers`：代理服务器列表

`servers` 至少需要一项。每个服务器包含：

| 字段 | 类型 | 必填 | 说明 |
|---|---:|---:|---|
| `name` | 字符串 | 是 | 本地唯一标识，会出现在日志和状态文件中。 |
| `token` | 字符串 | 是 | po0.io 服务器授权 Token，属于敏感信息。 |

多服务器示例：

```yaml
servers:
  - name: guangzhou-primary
    token: "TOKEN_ONE"
  - name: hongkong-backup
    token: "TOKEN_TWO"
```

运行行为：

- 每个服务器独立请求和记录结果。
- 成功项不会因其他服务器失败而重复调用。
- 失败项按照 `detection` 中的重试参数继续请求。
- 最新 IP 的所有服务器都成功后，才会发送 Telegram 成功通知。
- 新增一个从未同步过的 `name` 后，下一次成功检测 IP 时会立即同步它。
- 仅替换 Token 而保留原 `name` 时，状态可能仍认为该名称已经同步。需要立即强制同步时，使用一个新的唯一 `name` 后 reload；不要手工修改 `state.json`。

## 6. `telegram`：通知设置

| 字段 | 类型 | 默认值 | 说明 |
|---|---:|---:|---|
| `enabled` | 布尔值 | `false` | `true` 启用通知，`false` 完全跳过 Telegram 请求。 |
| `api_base` | 字符串 | `https://api.telegram.org` | Telegram Bot API 基础地址。使用反向代理时可修改。 |
| `bot_token` | 字符串 | 空 | BotFather 生成的 Bot Token，属于敏感信息。 |
| `chat_id` | 字符串 | 空 | 接收通知的用户、群组或频道 ID，建议始终加引号。 |
| `timeout_seconds` | 正整数 | `15` | 单次通知请求超时时间，单位秒。 |

当 `enabled: true` 时，`api_base`、`bot_token` 和 `chat_id` 必须有效且非空。当 `enabled: false` 时，程序不会验证 Token 是否可用，也不会发送首次同步通知。

Telegram 发送失败不会回滚已经成功的白名单更新；通知会单独持久化并按照同一套退避参数重试。极端情况下，如果通知已被 Telegram 接收、但程序在保存成功状态前断电，重启后可能重复发送一次。

安全建议：

- Bot Token 泄露后应立即在 BotFather 中撤销并重新生成。
- 配置文件保持 `0600`，不要在排障时直接输出完整文件。
- 可用 `awk` 或编辑器只查看 `enabled`，不要把 Token 复制到工单或聊天。

## 7. `detection`：检测、历史和重试策略

| 字段 | 类型 | 默认值 | 合法范围/关系 | 说明 |
|---|---:|---:|---|---|
| `fast_interval_seconds` | 正整数 | `300` | 最大 31,536,000 | 高频检测间隔，默认 5 分钟。 |
| `slow_interval_seconds` | 正整数 | `1800` | 最大 31,536,000 | 低风险阶段间隔，默认 30 分钟。 |
| `slow_until_percent` | 整数 | `75` | `1..=100` | 从上次变化开始，预测周期前多少百分比使用慢速检测。 |
| `history_sample_size` | 正整数 | `10` | 最大 10,000 | 计算中位数时最多取最近多少条变化间隔。 |
| `retention_days` | 正整数 | `7` | 最大 36,500 | 历史记录保留天数。 |
| `retry_initial_seconds` | 正整数 | `30` | 不大于最大重试间隔 | 白名单和 Telegram 首次失败后的等待时间。 |
| `retry_max_seconds` | 正整数 | `900` | 最大 31,536,000 | 指数退避上限，默认 15 分钟。 |

动态检测逻辑：

1. 没有历史变化间隔时，每 `fast_interval_seconds` 查询一次。
2. 有历史后，取保留期内最近 `history_sample_size` 条间隔的中位数作为典型周期。
3. 从最近一次 IP 变化起，到典型周期的 `slow_until_percent` 之前，使用 `slow_interval_seconds`。
4. 达到该百分比后，持续使用 `fast_interval_seconds`，直到检测到下一次变化。

例如典型周期为 6 小时、阈值为 75%：前 4.5 小时每 30 分钟查询，之后每 5 分钟查询。

重试间隔按 `30、60、120、240……` 秒指数增长，达到 `retry_max_seconds` 后保持上限。多个失败服务器分别保存自己的次数和下次重试时间。

维护建议：

- `slow_interval_seconds` 通常应大于 `fast_interval_seconds`。
- 不建议把快速间隔设得低于 60 秒，以免对公网接口造成不必要压力。
- 缩短 `retention_days` 会在后续检测时自动清理过期历史，不会立刻重写已有记录。

## 8. `storage`：状态和历史文件

| 字段 | 类型 | ImmortalWrt 推荐值 | 说明 |
|---|---:|---|---|
| `state_file` | 路径 | `/etc/po0-auto/data/state.json` | 当前 IPv4、预测周期、服务器同步和通知重试状态。 |
| `history_file` | 路径 | `/etc/po0-auto/data/history.json` | IPv4 变化历史及变化间隔。 |

两个路径不能相同。建议使用绝对路径，并放在只有 root 可访问的持久化目录中。

重要：程序内置默认路径是 `/var/lib/po0-auto`，但 ImmortalWrt/OpenWrt 通常将 `/var` 链接到 `/tmp`。如果省略 `storage` 或继续使用 `/var/lib`，路由器重启后状态和历史会丢失。生产配置必须显式使用持久化路径。

首次准备推荐目录：

```sh
mkdir -p /etc/po0-auto/data
chmod 0700 /etc/po0-auto/data
chown root:root /etc/po0-auto/data
```

已安装旧配置并使用 `/var/lib/po0-auto` 时，按以下顺序迁移，避免丢失当前 IP 和同步状态：

```sh
/etc/init.d/po0-auto stop
mkdir -p /etc/po0-auto/data
chmod 0700 /etc/po0-auto/data
chown root:root /etc/po0-auto/data
cp /var/lib/po0-auto/state.json /etc/po0-auto/data/state.json
cp /var/lib/po0-auto/history.json /etc/po0-auto/data/history.json
chmod 0600 /etc/po0-auto/data/state.json /etc/po0-auto/data/history.json
vi /etc/po0-auto/config.yaml
/usr/bin/po0-auto --config /etc/po0-auto/config.yaml --check-config
/etc/init.d/po0-auto start
logread -e po0-auto
```

编辑配置时将两个路径分别改成 `/etc/po0-auto/data/state.json` 和 `/etc/po0-auto/data/history.json`。确认新目录工作正常后，旧的 `/var/lib/po0-auto` 会在下次重启时自动消失，无需手工清理。

程序使用同目录临时文件和原子替换保存 JSON。新建文件权限为 `0600`；若 JSON 损坏，程序保留原文件并创建 `.corrupt-时间戳` 副本，然后安全退出。

不要在程序运行时直接编辑 `state.json` 或 `history.json`。备份或迁移时先停止服务：

```sh
/etc/init.d/po0-auto stop
cp /etc/po0-auto/data/state.json /安全的备份位置/state.json
cp /etc/po0-auto/data/history.json /安全的备份位置/history.json
/etc/init.d/po0-auto start
```

## 9. `user_agent`：HTTP 客户端标识

| 字段 | 类型 | 默认值 | 说明 |
|---|---:|---:|---|
| `user_agent` | 非空字符串 | 当前程序名和版本 | 所有公网 IP、白名单和 Telegram HTTP 请求使用的 User-Agent。 |

通常无需修改。使用企业代理或需要区分多台设备时，可以设置为不含凭据的标识：

```yaml
user_agent: po0-auto/0.1.0-immortalwrt-home
```

不要在 User-Agent 中放入 Token、家庭地址或其他敏感信息。

## 10. 配置变更影响速查

| 变更 | 是否需要 reload | 是否立即产生外部请求 |
|---|---:|---|
| 调整查询源或检测间隔 | 是 | reload 后立即执行一次公网 IP 检测。 |
| 新增服务器名称 | 是 | 下一次成功检测后同步该服务器。 |
| 修改已有名称的 Token | 是 | 可能不会立即重同步，建议同时使用新名称。 |
| 启用 Telegram | 是 | 不会补发过去已经完成的通知；下一次完整同步后通知。 |
| 修改状态文件路径 | 是 | 不会自动迁移旧文件，必须停服务后手工复制。 |
| 缩短历史保留期 | 是 | 后续检测时清理过期数据。 |

每次变更后都应依次执行配置校验、reload 和日志检查。
