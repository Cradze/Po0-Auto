# po0-auto

面向 x86_64 ImmortalWrt 的轻量级公网 IPv4 监测与 po0.io 白名单更新服务。程序按顺序查询多个公网 IP 接口，在 IP 变化后独立更新多台服务器，并在全部更新成功后发送 Telegram 通知。

生产部署请直接参阅 [`DEPLOYMENT.md`](DEPLOYMENT.md)。
全部配置字段说明见 [`CONFIGURATION.md`](CONFIGURATION.md)。

## 安全提示

`idea.md` 中曾出现真实 Telegram Bot Token，应视为已经泄露。请先在 BotFather 中撤销并生成新 Token，不要继续使用该值。示例配置不包含真实凭据；生产配置应保持 `0600` 权限。

程序不会在日志中输出服务器 Token、Telegram Token 或带敏感查询参数的 URL。状态目录会设置为 `0700`，状态文件设置为 `0600`。

## 本地开发

需要 Rust 1.85 或更高版本：

```sh
cargo test
cargo build --release
```

检查配置语法但不启动服务：

```sh
cargo run -- --config config.example.yaml --check-config
```

日志级别可通过 `RUST_LOG` 设置，例如 `RUST_LOG=debug`。即使使用 debug 日志也不会输出 Token。

## x86_64 ImmortalWrt 构建

ImmortalWrt 使用 musl。先安装 Rust 的 musl 目标，再下载与设备版本匹配的 x86_64 ImmortalWrt/OpenWrt SDK，并将 SDK 工具链的 `bin` 目录加入 `PATH`。确认以下命令存在：

```sh
x86_64-openwrt-linux-musl-gcc --version
```

然后执行：

```sh
rustup target add x86_64-unknown-linux-musl
export CC_x86_64_unknown_linux_musl=x86_64-openwrt-linux-musl-gcc
export AR_x86_64_unknown_linux_musl=x86_64-openwrt-linux-musl-ar
cargo build --release --target x86_64-unknown-linux-musl
```

产物位于 `target/x86_64-unknown-linux-musl/release/po0-auto`。部署前可用 `file` 和 `readelf -d` 检查架构及动态依赖。

## 安装到 ImmortalWrt

将二进制、示例配置和 init 脚本复制到路由器后，以 root 执行：

```sh
install -m 0755 po0-auto /usr/bin/po0-auto
install -d -m 0700 /etc/po0-auto /etc/po0-auto/data
install -m 0600 config.example.yaml /etc/po0-auto/config.yaml
install -m 0755 po0-auto.init /etc/init.d/po0-auto
vi /etc/po0-auto/config.yaml
/usr/bin/po0-auto --config /etc/po0-auto/config.yaml --check-config
/etc/init.d/po0-auto enable
/etc/init.d/po0-auto start
```

查看日志：

```sh
logread -e po0-auto
```

修改配置后执行 `/etc/init.d/po0-auto reload`。procd 会在异常退出时拉起程序。

## 运行行为

- 首次成功获取公网 IPv4 后立即更新全部服务器白名单。
- 单个服务器失败只重试该服务器，退避时间从 30 秒指数增长至最多 15 分钟；重启后继续未完成任务。
- 如果重试期间公网 IP 再次变化，旧任务会被最新 IP 的任务替代。
- 无变化历史时每 5 分钟查询；有历史后，典型周期前 75% 每 30 分钟查询，之后每 5 分钟查询。
- 典型周期取最近 7 天、最多 10 个变化间隔的中位数。状态文件损坏时程序保留原文件及时间戳副本并安全退出。
- Telegram 开启后，只在最新 IP 的全部服务器更新成功后通知；通知失败同样持久化并退避重试。

配置示例见 [`config.example.yaml`](config.example.yaml)，字段说明、默认值和维护建议见 [`CONFIGURATION.md`](CONFIGURATION.md)。
