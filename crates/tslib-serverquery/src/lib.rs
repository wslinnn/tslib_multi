//! TeamSpeak ServerQuery client (TCP text protocol).
//!
//! ServerQuery is independent of the voice/UDP client: it logs in as a query
//! user and runs raw commands (`serverinfo`, `clientlist`, `channelcreate`,
//! ...). SSH/WebQuery transports are not implemented; use a tunnel or the
//! regular TCP endpoint.
//!
//! ```no_run
//! # async fn example() -> Result<(), tslib_serverquery::Error> {
//! let mut query = tslib_serverquery::ServerQueryClient::connect("127.0.0.1:10011").await?;
//! query.login("serveradmin", "password").await?;
//! let info = query.execute("serverinfo").await?;
//! # Ok(())
//! # }
//! ```

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;

/// ServerQuery error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("server rejected command: id={id} msg={message}")]
    Server { id: u32, message: String },
}

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// A connected ServerQuery client.
pub struct ServerQueryClient {
    reader: BufReader<ReadHalf<TcpStream>>,
    writer: WriteHalf<TcpStream>,
    timeout: Duration,
}

impl ServerQueryClient {
    /// Connect and consume the two-line banner.
    pub async fn connect(address: impl tokio::net::ToSocketAddrs) -> Result<Self> {
        let stream = TcpStream::connect(address).await?;
        let (read, write) = tokio::io::split(stream);
        let mut reader = BufReader::new(read);

        // Banner: "TS3" and a welcome line.
        let mut banner = String::new();
        reader.read_line(&mut banner).await?;
        banner.clear();
        reader.read_line(&mut banner).await?;

        Ok(Self {
            reader,
            writer: write,
            timeout: Duration::from_secs(10),
        })
    }

    /// Override the per-command read timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Login as a query user.
    pub async fn login(&mut self, user: &str, password: &str) -> Result<()> {
        self.execute(&format!(
            "login {} {}",
            escape(user),
            escape(password)
        ))
        .await
        .map(|_| ())
    }

    /// Execute a raw command and return the response lines before the final
    /// `error id=...` line.
    pub async fn execute(&mut self, command: &str) -> Result<Vec<String>> {
        let mut line = command.replace(['\r', '\n'], "");
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await?;

        let mut response = Vec::new();
        loop {
            let mut raw = String::new();
            let read = tokio::time::timeout(self.timeout, self.reader.read_line(&mut raw));
            let bytes = match read.await {
                Ok(result) => result?,
                Err(_) => {
                    return Err(Error::Protocol("timed out waiting for response".into()));
                }
            };
            if bytes == 0 {
                return Err(Error::Protocol("connection closed".into()));
            }
            let trimmed = raw.trim_matches(['\r', '\n']).to_string();
            if let Some(error) = parse_error(&trimmed) {
                return match error {
                    (0, _) => Ok(response),
                    (id, message) => Err(Error::Server { id, message }),
                };
            }
            response.push(trimmed);
        }
    }

    /// Execute a command with escaped key/value arguments.
    pub async fn execute_with_args(
        &mut self,
        command: &str,
        args: &[(&str, &str)],
    ) -> Result<Vec<String>> {
        let mut line = command.to_string();
        for (key, value) in args {
            line.push(' ');
            line.push_str(key);
            line.push('=');
            line.push_str(&escape(value));
        }
        self.execute(&line).await
    }

    /// Send `quit` and close the connection.
    pub async fn quit(mut self) -> Result<()> {
        let _ = self.execute("quit").await;
        let _ = self.writer.shutdown().await;
        Ok(())
    }
}

fn parse_error(line: &str) -> Option<(u32, String)> {
    if !line.starts_with("error id=") {
        return None;
    }
    let rest = line.trim_start_matches("error id=");
    let mut parts = rest.splitn(2, ' ');
    let id = parts.next()?.parse().ok()?;
    let message = parts
        .next()
        .map(|value| value.trim_start_matches("msg=").to_string())
        .unwrap_or_default();
    Some((id, message))
}

/// Escape a ServerQuery argument value.
pub fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            ' ' => escaped.push_str("\\s"),
            '/' => escaped.push_str("\\/"),
            '|' => escaped.push_str("\\p"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    #[test]
    fn escapes_special_characters() {
        assert_eq!(escape("a b/c|d\\e"), "a\\sb\\/c\\pd\\\\e");
    }

    #[test]
    fn parses_error_lines() {
        assert_eq!(parse_error("error id=0 msg=ok"), Some((0, "ok".into())));
        assert_eq!(
            parse_error("error id=2568 msg=insufficient\\spermissions"),
            Some((2568, "insufficient\\spermissions".into()))
        );
        assert_eq!(parse_error("not an error"), None);
    }

    #[tokio::test]
    async fn login_and_execute_against_mock() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(b"TS3\n\r").await.unwrap();
            socket
                .write_all(b"Welcome to the TeamSpeak 3 ServerQuery interface\n\r")
                .await
                .unwrap();

            let mut buffer = vec![0u8; 256];
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(String::from_utf8_lossy(&buffer[..read]).starts_with("login "));
            socket.write_all(b"error id=0 msg=ok\n\r").await.unwrap();

            let read = socket.read(&mut buffer).await.unwrap();
            assert!(String::from_utf8_lossy(&buffer[..read]).starts_with("serverinfo"));
            socket
                .write_all(b"virtualserver_name=Test\n\rerror id=0 msg=ok\n\r")
                .await
                .unwrap();
        });

        let mut client = ServerQueryClient::connect(address).await.unwrap();
        client.login("serveradmin", "secret").await.unwrap();
        let response = client.execute("serverinfo").await.unwrap();
        assert!(response.iter().any(|line| line.contains("virtualserver_name")));

        server.await.unwrap();
    }
}
