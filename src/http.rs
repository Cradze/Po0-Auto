use std::{net::Ipv4Addr, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};

use crate::config::{Config, IpResponseKind, IpSourceConfig, ServerConfig, TelegramConfig};

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
}

impl ApiClient {
    pub fn new(config: &Config) -> Result<Self> {
        let client = Client::builder()
            .user_agent(&config.user_agent)
            .build()
            .context("无法创建 HTTP 客户端")?;
        Ok(Self { client })
    }

    pub async fn public_ipv4(&self, sources: &[IpSourceConfig]) -> Result<(Ipv4Addr, String)> {
        let mut errors = Vec::with_capacity(sources.len());
        for source in sources {
            match self.public_ipv4_from(source).await {
                Ok(ipv4) => return Ok((ipv4, source.name.clone())),
                Err(error) => errors.push(format!("{}: {error:#}", source.name)),
            }
        }
        bail!("所有公网 IPv4 接口均失败: {}", errors.join("; "))
    }

    async fn public_ipv4_from(&self, source: &IpSourceConfig) -> Result<Ipv4Addr> {
        let response = self
            .client
            .get(&source.url)
            .timeout(source.timeout())
            .send()
            .await
            .map_err(|error| safe_request_error("公网 IP", &error))?;
        if !response.status().is_success() {
            bail!("返回 HTTP {}", response.status());
        }
        let body = response
            .text()
            .await
            .map_err(|error| safe_request_error("读取公网 IP 响应", &error))?;
        parse_public_ipv4(&body, source.response)
    }

    pub async fn update_whitelist(
        &self,
        base_url: &str,
        timeout: Duration,
        server: &ServerConfig,
    ) -> Result<()> {
        let mut url = Url::parse(base_url).context("白名单 URL 无效")?;
        url.query_pairs_mut()
            .append_pair("action", "add")
            .append_pair("token", &server.token);
        let response = self
            .client
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .map_err(|error| safe_request_error("白名单更新", &error))?;
        ensure_success("白名单更新", response.status())
    }

    pub async fn send_telegram(&self, config: &TelegramConfig, text: &str) -> Result<()> {
        let base = config.api_base.trim_end_matches('/');
        let url = format!("{base}/bot{}/sendMessage", config.bot_token);
        let body = TelegramMessage {
            chat_id: &config.chat_id,
            text,
        };
        let response = self
            .client
            .post(url)
            .timeout(Duration::from_secs(config.timeout_seconds))
            .json(&body)
            .send()
            .await
            .map_err(|error| safe_request_error("Telegram 通知", &error))?;
        ensure_success("Telegram 通知", response.status())
    }
}

fn ensure_success(operation: &str, status: StatusCode) -> Result<()> {
    if status.is_success() {
        Ok(())
    } else {
        bail!("{operation}返回 HTTP {status}")
    }
}

fn safe_request_error(operation: &str, error: &reqwest::Error) -> anyhow::Error {
    let reason = if error.is_timeout() {
        "请求超时"
    } else if error.is_connect() {
        "连接失败"
    } else if error.is_request() {
        "请求构造失败"
    } else if error.is_body() || error.is_decode() {
        "响应读取失败"
    } else {
        "网络错误"
    };
    anyhow!("{operation}{reason}")
}

#[derive(Deserialize)]
struct BilibiliResponse {
    code: i64,
    data: BilibiliData,
}

#[derive(Deserialize)]
struct BilibiliData {
    addr: String,
}

#[derive(Serialize)]
struct TelegramMessage<'a> {
    chat_id: &'a str,
    text: &'a str,
}

pub fn parse_public_ipv4(body: &str, kind: IpResponseKind) -> Result<Ipv4Addr> {
    let value = match kind {
        IpResponseKind::PlainText => body.trim(),
        IpResponseKind::BilibiliJson => {
            let response: BilibiliResponse =
                serde_json::from_str(body).context("Bilibili 响应不是预期的 JSON")?;
            if response.code != 0 {
                bail!("Bilibili 返回业务错误码 {}", response.code);
            }
            return parse_and_validate_ipv4(response.data.addr.trim());
        }
    };
    parse_and_validate_ipv4(value)
}

fn parse_and_validate_ipv4(value: &str) -> Result<Ipv4Addr> {
    let ipv4: Ipv4Addr = value.parse().context("响应不是合法 IPv4")?;
    if !is_public_ipv4(ipv4) {
        bail!("响应不是公网 IPv4");
    }
    Ok(ipv4)
}

pub fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _d] = ip.octets();
    !matches!(
        (a, b, c),
        (0, _, _)
            | (10, _, _)
            | (127, _, _)
            | (100, 64..=127, _)
            | (169, 254, _)
            | (172, 16..=31, _)
            | (192, 0, 0)
            | (192, 0, 2)
            | (192, 88, 99)
            | (192, 168, _)
            | (198, 18..=19, _)
            | (198, 51, 100)
            | (203, 0, 113)
            | (224..=255, _, _)
    )
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    use super::*;
    use crate::config::{Config, IpSourceConfig};

    #[test]
    fn parses_plain_and_bilibili_responses() {
        assert_eq!(
            parse_public_ipv4(" 8.8.8.8\n", IpResponseKind::PlainText).unwrap(),
            Ipv4Addr::new(8, 8, 8, 8)
        );
        assert_eq!(
            parse_public_ipv4(
                r#"{"code":0,"data":{"addr":"1.1.1.1"}}"#,
                IpResponseKind::BilibiliJson
            )
            .unwrap(),
            Ipv4Addr::new(1, 1, 1, 1)
        );
    }

    #[test]
    fn rejects_non_ipv4_and_non_public_ranges() {
        for value in [
            "2001:4860:4860::8888",
            "10.0.0.1",
            "100.64.0.1",
            "192.168.1.1",
            "203.0.113.4",
            "255.255.255.255",
            "1.1.1.1 extra",
        ] {
            assert!(parse_public_ipv4(value, IpResponseKind::PlainText).is_err());
        }
    }

    #[tokio::test]
    async fn falls_back_after_an_invalid_response() {
        let (bad_url, bad_request) = mock_once(200, "not an ip");
        let (good_url, good_request) = mock_once(200, "8.8.4.4\n");
        let sources = vec![
            IpSourceConfig {
                name: "bad".to_owned(),
                url: bad_url,
                response: IpResponseKind::PlainText,
                timeout_seconds: 2,
            },
            IpSourceConfig {
                name: "good".to_owned(),
                url: good_url,
                response: IpResponseKind::PlainText,
                timeout_seconds: 2,
            },
        ];
        let client = ApiClient::new(&Config::default()).unwrap();
        let (ipv4, source) = client.public_ipv4(&sources).await.unwrap();
        assert_eq!(ipv4, Ipv4Addr::new(8, 8, 4, 4));
        assert_eq!(source, "good");
        assert!(bad_request.recv().unwrap().starts_with("GET / HTTP/1.1"));
        assert!(good_request.recv().unwrap().starts_with("GET / HTTP/1.1"));
    }

    #[tokio::test]
    async fn sends_expected_firewall_and_telegram_requests() {
        let (firewall_url, firewall_request) = mock_once(204, "");
        let client = ApiClient::new(&Config::default()).unwrap();
        let server = ServerConfig {
            name: "primary".to_owned(),
            token: "secret-token".to_owned(),
        };
        client
            .update_whitelist(&firewall_url, Duration::from_secs(2), &server)
            .await
            .unwrap();
        let request = firewall_request.recv().unwrap();
        assert!(request.starts_with("GET /?action=add&token=secret-token HTTP/1.1"));

        let (telegram_base, telegram_request) = mock_once(200, r#"{"ok":true}"#);
        let telegram = TelegramConfig {
            enabled: true,
            api_base: telegram_base,
            bot_token: "bot-token".to_owned(),
            chat_id: "-100123".to_owned(),
            timeout_seconds: 2,
        };
        client.send_telegram(&telegram, "hello").await.unwrap();
        let request = telegram_request.recv().unwrap();
        assert!(request.starts_with("POST /botbot-token/sendMessage HTTP/1.1"));
        assert!(request.contains(r#"{"chat_id":"-100123","text":"hello"}"#));
    }

    fn mock_once(status: u16, response_body: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        bytes.extend_from_slice(&buffer[..read]);
                        if request_is_complete(&bytes) {
                            break;
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        break;
                    }
                    Err(error) => panic!("mock read failed: {error}"),
                }
            }
            sender.send(String::from_utf8(bytes).unwrap()).unwrap();
            let reason = if status == 204 { "No Content" } else { "OK" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{address}"), receiver)
    }

    fn request_is_complete(request: &[u8]) -> bool {
        let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
            return false;
        };
        let header_end = header_end + 4;
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        request.len() >= header_end + content_length
    }
}
