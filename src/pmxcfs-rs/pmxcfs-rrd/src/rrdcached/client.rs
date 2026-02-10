use super::create::*;
use super::errors::RRDCachedClientError;
use super::now::now_timestamp;
use super::parsers::*;
use super::sanitisation::check_rrd_path;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::io::BufReader;

/// A client to interact with a RRDCached server over Unix socket.
///
/// This is a trimmed version containing only the methods we actually use:
/// - connect_unix() - Connect to rrdcached
/// - create() - Create new RRD files
/// - update() - Update RRD data
/// - flush_all() - Flush pending updates
#[derive(Debug)]
pub struct RRDCachedClient<T = UnixStream> {
    stream: BufReader<T>,
}

impl RRDCachedClient<UnixStream> {
    /// Connect to a RRDCached server over a Unix socket.
    ///
    /// Connection attempts timeout after 10 seconds to prevent indefinite hangs
    /// if the rrdcached daemon is stuck or unresponsive.
    pub async fn connect_unix(addr: &str) -> Result<Self, RRDCachedClientError> {
        let connect_future = UnixStream::connect(addr);
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connect_future
        )
        .await
        .map_err(|_| RRDCachedClientError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Connection to rrdcached timed out after 10 seconds"
        )))??;
        let stream = BufReader::new(stream);
        Ok(Self { stream })
    }
}

impl<T> RRDCachedClient<T>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    fn assert_response_code(&self, code: i64, message: &str) -> Result<(), RRDCachedClientError> {
        if code < 0 {
            Err(RRDCachedClientError::UnexpectedResponse(
                code,
                message.to_string(),
            ))
        } else {
            Ok(())
        }
    }

    async fn read_line(&mut self) -> Result<String, RRDCachedClientError> {
        let mut line = String::new();
        self.stream.read_line(&mut line).await?;
        Ok(line)
    }

    async fn read_n_lines(&mut self, n: usize) -> Result<Vec<String>, RRDCachedClientError> {
        let mut lines = Vec::with_capacity(n);
        for _ in 0..n {
            let line = self.read_line().await?;
            lines.push(line);
        }
        Ok(lines)
    }

    async fn write_command_and_read_response(
        &mut self,
        command: &str,
    ) -> Result<(String, Vec<String>), RRDCachedClientError> {
        self.stream.write_all(command.as_bytes()).await?;

        // Read response header line
        let first_line = self.read_line().await?;
        let (code, message) = parse_response_line(&first_line)?;
        self.assert_response_code(code, message)?;

        // Parse number of following lines from message
        let nb_lines: usize = message.parse().unwrap_or(0);

        // Read the following lines if any
        let lines = self.read_n_lines(nb_lines).await?;

        Ok((message.to_string(), lines))
    }

    async fn send_command(&mut self, command: &str) -> Result<(usize, String), RRDCachedClientError> {
        let (message, _lines) = self.write_command_and_read_response(command).await?;
        let nb_lines: usize = message.parse().unwrap_or(0);
        Ok((nb_lines, message))
    }

    /// Create a new RRD file
    ///
    /// # Arguments
    /// * `arguments` - CreateArguments containing path, data sources, and archives
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(RRDCachedClientError)` if creation fails
    pub async fn create(&mut self, arguments: CreateArguments) -> Result<(), RRDCachedClientError> {
        arguments.validate()?;

        // Build CREATE command string
        let arguments_str = arguments.to_str();
        let mut command = String::with_capacity(7 + arguments_str.len() + 1);
        command.push_str("CREATE ");
        command.push_str(&arguments_str);
        command.push('\n');

        let (_, message) = self.send_command(&command).await?;

        // -1 means success for CREATE (file created)
        // Positive number means error
        if !message.starts_with('-') {
            return Err(RRDCachedClientError::UnexpectedResponse(
                0,
                format!("CREATE command failed: {message}"),
            ));
        }

        Ok(())
    }

    /// Flush all pending RRD updates to disk
    ///
    /// This ensures all buffered updates are written to RRD files.
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(RRDCachedClientError)` if flush fails
    pub async fn flush_all(&mut self) -> Result<(), RRDCachedClientError> {
        let _ = self.send_command("FLUSHALL\n").await?;
        Ok(())
    }

    /// Update an RRD with a list of values at a specific timestamp
    ///
    /// The order of values must match the order of data sources in the RRD.
    ///
    /// # Arguments
    /// * `path` - Path to RRD file (without .rrd extension)
    /// * `timestamp` - Optional Unix timestamp (None = current time)
    /// * `data` - Vector of values, one per data source
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(RRDCachedClientError)` if update fails
    ///
    /// # Example
    /// ```ignore
    /// client.update("myfile", None, vec![1.0, 2.0, 3.0]).await?;
    /// ```
    pub async fn update(
        &mut self,
        path: &str,
        timestamp: Option<usize>,
        data: Vec<f64>,
    ) -> Result<(), RRDCachedClientError> {
        // Validate inputs
        if data.is_empty() {
            return Err(RRDCachedClientError::InvalidCreateDataSerie(
                "data is empty".to_string(),
            ));
        }
        check_rrd_path(path)?;

        // Build UPDATE command: "UPDATE path.rrd timestamp:value1:value2:...\n"
        let timestamp_str = match timestamp {
            Some(ts) => ts.to_string(),
            None => now_timestamp()?.to_string(),
        };

        let data_str = data
            .iter()
            .map(|f| {
                if f.is_nan() {
                    "U".to_string()
                } else {
                    f.to_string()
                }
            })
            .collect::<Vec<String>>()
            .join(":");

        let mut command = String::with_capacity(
            7 + path.len() + 5 + timestamp_str.len() + 1 + data_str.len() + 1,
        );
        command.push_str("UPDATE ");
        command.push_str(path);
        command.push_str(".rrd ");
        command.push_str(&timestamp_str);
        command.push(':');
        command.push_str(&data_str);
        command.push('\n');

        // Send command
        let _ = self.send_command(&command).await?;
        Ok(())
    }
}
