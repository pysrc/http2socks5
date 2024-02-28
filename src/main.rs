use std::{fs::File, io::Read, str::FromStr};

use serde::{Deserialize, Serialize};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}};

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Config {
    bind: String,
    socks5: String,
}

impl Config {
    fn from_file(filename: &str) -> Self {
        let f = File::open(filename);
        match f {
            Ok(mut file) => {
                let mut c = String::new();
                file.read_to_string(&mut c).unwrap();
                let cfg: Config = serde_yaml::from_str(&c).unwrap();
                cfg
            }
            Err(e) => {
                panic!("error {}", e)
            }
        }
    }
}

async fn read_until(mut conn: TcpStream, stop_ch: char) -> (TcpStream, String) {
    let mut _buf = String::new();
    loop {
        let ch = conn.read_u8().await.unwrap() as char;
        if ch == stop_ch {
            return (conn, _buf);
        }
        _buf.push(ch);
    }
}

async fn get_sock5_conn(socks5addr: &str, dst: &str) -> Option<TcpStream> {
        // 连接到SOCKS5代理服务器
        let mut stream = TcpStream::connect(socks5addr).await.unwrap();
    
        // 发送握手消息，选择支持的认证方法
        stream.write_all(&[5, 1, 0]).await.unwrap();
        
        // 读取代理服务器的响应，确认认证方法
        let mut buf = [0; 2];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [5, 0]); // 检查代理服务器是否接受无需认证
        
        // 判断是域名还是ip
        match std::net::SocketAddr::from_str(dst) {
            Ok(addr) => {
                match addr {
                    std::net::SocketAddr::V4(v4addr) => {
                        // 发送连接请求消息
                        let [a, b, c, d] = v4addr.ip().octets();
                        let [e, f] = v4addr.port().to_be_bytes();
                        stream.write_all(&[5, 1, 0, 1, a, b, c, d, e, f]).await.unwrap();
                    }
                    _ => {
                        return None;
                    }
                }
            }
            Err(_) => {
                // 域名
                stream.write_all(&[5, 1, 0, 3]).await.unwrap();
                let mut _iter = dst.split(":");
                let _domain = _iter.next().unwrap();
                let _port: u16 = _iter.next().unwrap().parse().unwrap();
                stream.write_u8(_domain.len() as u8).await.unwrap();
                stream.write_all(_domain.as_bytes()).await.unwrap();
                stream.write_all(&_port.to_be_bytes()).await.unwrap();
            }
        }
        
        // 读取代理服务器的响应，确认连接是否建立成功
        let mut buf = [0; 10];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf[..4], [5, 0, 0, 1]); // 检查连接是否成功
        return Some(stream);
}

async fn handle(conn: TcpStream, socks5addr: &str) {
    let (conn, _method) = read_until(conn, ' ').await;
    if _method == "CONNECT" {
        // CONNECT www.baidu.com:443 HTTP/1.1
        // https
        let (mut conn, mut _domain) = read_until(conn, ' ').await;
        conn.read(&mut [0u8; 1024]).await.unwrap();
        log::info!("dst->https://{}", _domain);
        if !_domain.contains(":") {
            _domain.push_str(":443");
        }
        // 响应
        conn.write_all("HTTP/1.1 200 OK\r\n\r\n".as_bytes()).await.unwrap();
        // 转换为socks5代理
        if let Some(mut dst) = get_sock5_conn(socks5addr, &_domain).await {
            tokio::io::copy_bidirectional(&mut conn, &mut dst).await.unwrap();
        }
    } else {
        // GET http://127.0.0.1:8000/ HTTP/1.1
        // http
        let (mut conn, _) = read_until(conn, '/').await;
        _ = conn.read_u8().await.unwrap();
        let (mut conn, mut _domain) = read_until(conn, '/').await;
        log::info!("dst->http://{}", _domain);
        if !_domain.contains(":") {
            _domain.push_str(":80");
        }
        // 转换为socks5代理
        if let Some(mut dst) = get_sock5_conn(socks5addr, &_domain).await {
            dst.write_all(_method.as_bytes()).await.unwrap();
            dst.write_u8(' ' as u8).await.unwrap();
            dst.write_u8('/' as u8).await.unwrap();
            tokio::io::copy_bidirectional(&mut conn, &mut dst).await.unwrap();
        }
    }
}

#[tokio::main]
async fn main() {
    simple_logger::init_with_level(log::Level::Info).unwrap();
    let cfg = Config::from_file("config.yml");
    log::info!("{:?}", cfg);
    let listener = TcpListener::bind(cfg.bind).await.unwrap();
    loop {
        let (conn, _) = listener.accept().await.unwrap();
        let s5addr = cfg.socks5.clone();
        tokio::spawn(async move{
            handle(conn, &s5addr).await;
        });
    }
}
