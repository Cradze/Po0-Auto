# po0-auto 在 x86_64 ImmortalWrt 上的部署指南

本文档适用于 `po0-auto 0.1.0` 的 x86_64-musl 静态版本。以下命令分为“本地电脑执行”和“ImmortalWrt 执行”，不要混用。

全部配置字段、默认值和维护注意事项见 [`CONFIGURATION.md`](CONFIGURATION.md)。

## 1. 部署前确认

在 ImmortalWrt 上通过 SSH 执行：

```sh
uname -m
date
df -h /usr /etc /var
```

必须满足：

- `uname -m` 输出 `x86_64`。
- 系统日期和时间基本正确，否则 HTTPS 证书校验会失败。
- `/usr` 至少有约 5 MB 可用空间。
- 路由器能通过 WAN 访问 TCP 443，并且 DNS 解析正常。

本版本为 musl 静态 PIE，不需要在 ImmortalWrt 上安装 Rust、glibc 或其他动态库。

## 2. 准备凭据

部署前准备：

- 一个或多个 po0.io 服务器 Token。
- Telegram Chat ID。
- 一个新生成的 Telegram Bot Token。

`idea.md` 中曾出现过真实 Bot Token，该 Token 应视为已经泄露。请先在 BotFather 中撤销它并生成新 Token，不要将旧值写入生产配置。

配置文件不支持 `${ENV_VAR}` 替换，必须直接填写实际值。配置完成后会将文件权限设置为 `0600`。

## 3. 从本地电脑上传文件

在项目根目录的本地电脑上执行，将 `192.168.1.1` 替换成路由器管理地址：

```sh
scp target/x86_64-unknown-linux-musl/release/po0-auto root@192.168.188.1:/tmp/po0-auto
scp config.example.yaml root@192.168.188.1:/tmp/po0-auto-config.yaml
scp deploy/po0-auto.init root@192.168.188.1:/tmp/po0-auto.init
```

当前发布二进制的 SHA-256 为：

```text
bbb4696b771242963fa6f65b65851635486c74a1b4190c0f452de50919cccb7f
```

在 ImmortalWrt 上核验上传结果：

```sh
sha256sum /tmp/po0-auto
```

输出必须与上述值完全一致。如果不一致，不要继续部署，应重新上传文件。

## 4. 首次安装

以下命令全部在 ImmortalWrt 上以 root 执行：

```sh
/etc/init.d/po0-auto stop 2>/dev/null
mkdir -p /etc/po0-auto
mkdir -p /etc/po0-auto/data
cp /tmp/po0-auto /usr/bin/po0-auto
cp /tmp/po0-auto.init /etc/init.d/po0-auto
cp /tmp/po0-auto-config.yaml /etc/po0-auto/config.yaml
chmod 0755 /usr/bin/po0-auto
chmod 0755 /etc/init.d/po0-auto
chmod 0700 /etc/po0-auto
chmod 0700 /etc/po0-auto/data
chmod 0600 /etc/po0-auto/config.yaml
chown root:root /usr/bin/po0-auto
chown root:root /etc/init.d/po0-auto
chown -R root:root /etc/po0-auto
```

确认二进制可以执行：

```sh
/usr/bin/po0-auto --version
```

预期输出：

```text
po0-auto 0.1.0
```

如果出现 `Exec format error`，说明设备不是 x86_64 或上传了错误架构的文件。

## 5. 编辑生产配置

在 ImmortalWrt 上执行：

```sh
vi /etc/po0-auto/config.yaml
```

至少修改 `servers` 和 `telegram`。示例：

```yaml
servers:
  - name: primary
    token: 替换为第一台服务器Token
  - name: backup
    token: 替换为第二台服务器Token

telegram:
  enabled: true
  api_base: https://api.telegram.org
  bot_token: 替换为新生成的BotToken
  chat_id: "-1001234567890"
  timeout_seconds: 15
```

注意事项：

- 每个服务器的 `name` 必须唯一。
- 负数 Chat ID 建议放在引号中。
- 保留示例中的 `/etc/po0-auto/data` 持久化路径；ImmortalWrt 的 `/var` 通常链接到 `/tmp`，不能保存跨重启的状态和历史。
- Telegram 暂时不用时可设置 `enabled: false`，但服务器列表仍必须至少有一项。

全部配置项说明见 [`CONFIGURATION.md`](CONFIGURATION.md)。

保存后再次收紧权限并执行只读配置校验：

```sh
chmod 0600 /etc/po0-auto/config.yaml
/usr/bin/po0-auto --config /etc/po0-auto/config.yaml --check-config
```

预期输出：

```text
配置有效: /etc/po0-auto/config.yaml
```

`--check-config` 不访问公网、不更新白名单，也不创建状态文件。

## 6. 启动和开机自启

在 ImmortalWrt 上执行：

```sh
/etc/init.d/po0-auto enable
/etc/init.d/po0-auto start
sleep 3
logread -e po0-auto
```

也可以实时观察日志：

```sh
logread -f
```

首次成功获取公网 IPv4 后，程序会立即更新全部服务器白名单。某个服务器失败时只重试失败项；所有服务器成功后才发送 Telegram 成功通知。

确认进程和状态文件：

```sh
pidof po0-auto
ls -l /etc/po0-auto/data
cat /etc/po0-auto/data/state.json
cat /etc/po0-auto/data/history.json
```

应满足：

- `pidof po0-auto` 输出一个进程号。
- `state.json` 中的 `current_ipv4` 是当前公网 IPv4。
- `pending_sync` 和 `pending_notification` 在全部操作完成后均为 `null`。
- `state.json`、`history.json` 权限为 `-rw-------`。
- Telegram 收到来自 ImmortalWrt 的通知。

## 7. 常用运维命令

```sh
# 查看日志
logread -e po0-auto

# 修改配置后重载
/etc/init.d/po0-auto reload

# 重启
/etc/init.d/po0-auto restart

# 停止
/etc/init.d/po0-auto stop

# 启动
/etc/init.d/po0-auto start

# 取消开机自启
/etc/init.d/po0-auto disable
```

程序会响应 `SIGTERM` 并正常保存状态后退出。procd 会在程序异常退出时自动拉起。

## 8. 升级和回滚

上传新二进制到 `/tmp/po0-auto.new` 并核验新版 SHA-256，然后在 ImmortalWrt 上执行：

```sh
/etc/init.d/po0-auto stop
cp /usr/bin/po0-auto /usr/bin/po0-auto.previous
cp /tmp/po0-auto.new /usr/bin/po0-auto
chmod 0755 /usr/bin/po0-auto
chown root:root /usr/bin/po0-auto
/usr/bin/po0-auto --version
/usr/bin/po0-auto --config /etc/po0-auto/config.yaml --check-config
/etc/init.d/po0-auto start
logread -e po0-auto
```

升级不会主动删除 `/etc/po0-auto/data` 中的状态和历史。

如果新版无法启动，回滚：

```sh
/etc/init.d/po0-auto stop
cp /usr/bin/po0-auto.previous /usr/bin/po0-auto
chmod 0755 /usr/bin/po0-auto
/etc/init.d/po0-auto start
```

确认回滚成功后再删除 `/usr/bin/po0-auto.previous`。

## 9. 凭据或服务器配置变更

修改配置后执行：

```sh
/usr/bin/po0-auto --config /etc/po0-auto/config.yaml --check-config
/etc/init.d/po0-auto reload
```

添加新服务器时使用新的唯一 `name`，程序在下一次成功检测 IP 时会发现该服务器尚未同步并立即更新。

如果只替换了某个服务器的 Token、但保留原 `name`，当前状态可能认为该名称已经同步。要强制立即重新同步，可将该服务器改成一个新的唯一名称，然后 reload；不要直接编辑 `state.json`。

## 10. 故障排查

### 所有公网 IP 接口均失败

```sh
date
nslookup api.bilibili.com
nslookup ipinfo.io
logread -e po0-auto
```

检查 WAN、DNS、防火墙和系统时间。程序会按配置顺序自动 fallback，不会因为一次查询失败而错误更新 IP。

### HTTPS 请求持续失败

先检查 `date`。系统时间相差较大时，TLS 证书会被判定为无效。可重启系统 NTP 服务：

```sh
/etc/init.d/sysntpd restart
```

### 配置校验失败

根据错误信息检查 YAML 缩进、重复服务器名称、空 Token、URL 和检测时间参数。Tab 不应作为 YAML 缩进。

### 白名单一直重试

```sh
logread -e po0-auto
cat /etc/po0-auto/data/state.json
```

查看 `pending_sync.servers` 中对应服务器的 `last_error` 和 `attempts`。日志和状态文件不会保存 Token。

### Telegram 没有通知

确认全部服务器已经同步完成，并检查 `pending_notification`。Telegram 只在全部白名单更新成功后发送；通知请求失败时会从 30 秒开始指数退避，最长 15 分钟。

### 状态文件损坏

程序不会静默覆盖损坏文件，而是创建带 `.corrupt-时间戳` 的 `0600` 副本并退出。先停止服务、保存损坏文件，再决定修复或移走对应 JSON；移走 `state.json` 会让下次启动按首次运行处理并重新同步全部服务器。

## 11. 部署验收清单

- [ ] 设备架构为 `x86_64`，系统时间正确。
- [ ] 上传后二进制 SHA-256 校验一致。
- [ ] 已撤销 `idea.md` 中泄露的 Bot Token，并使用新 Token。
- [ ] 配置校验输出“配置有效”。
- [ ] 配置目录和状态目录为 `0700`，敏感文件为 `0600`。
- [ ] 状态文件位于持久化的 `/etc/po0-auto/data`，而不是 `/var` 或 `/tmp`。
- [ ] `pidof po0-auto` 能看到进程。
- [ ] 日志显示公网 IPv4 检测成功及各服务器白名单更新成功。
- [ ] Telegram 收到首次同步通知。
- [ ] `/etc/init.d/po0-auto enable` 已执行，重启路由器后服务能自动运行。
