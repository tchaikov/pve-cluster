//! IPC request types and parsing
//!
//! This module defines the IPC operation codes and request message types
//! used for communication between pmxcfs and client applications via libqb IPC.

/// IPC operation codes (must match C version for compatibility)
#[derive(Debug, Clone, Copy, PartialEq, Eq, num_enum::TryFromPrimitive)]
#[repr(i32)]
pub enum CfsIpcOp {
    GetFsVersion = 1,
    GetClusterInfo = 2,
    GetGuestList = 3,
    SetStatus = 4,
    GetStatus = 5,
    GetConfig = 6,
    LogClusterMsg = 7,
    GetClusterLog = 8,
    GetRrdDump = 10,
    GetGuestConfigProperty = 11,
    VerifyToken = 12,
    GetGuestConfigProperties = 13,
}

/// IPC request message
///
/// Represents deserialized IPC requests sent from clients via libqb IPC.
/// Each variant corresponds to an IPC operation code and contains the
/// deserialized request parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum IpcRequest {
    /// GET_FS_VERSION (op 1): Get filesystem version info
    GetFsVersion,

    /// GET_CLUSTER_INFO (op 2): Get cluster member list
    GetClusterInfo,

    /// GET_GUEST_LIST (op 3): Get VM/CT list
    GetGuestList,

    /// SET_STATUS (op 4): Update node status
    SetStatus { name: String, data: Vec<u8> },

    /// GET_STATUS (op 5): Get node status
    /// C format: name (256 bytes) + nodename (256 bytes)
    GetStatus {
        name: String,
        node_name: String,
    },

    /// GET_CONFIG (op 6): Read configuration file
    GetConfig { path: String },

    /// LOG_CLUSTER_MSG (op 7): Write to cluster log
    LogClusterMsg {
        priority: u8,
        ident: String,
        tag: String,
        message: String,
    },

    /// GET_CLUSTER_LOG (op 8): Read cluster log
    /// C struct has max_entries + 3 reserved u32s + user string
    GetClusterLog { max_entries: usize, user: String },

    /// GET_RRD_DUMP (op 10): Get RRD data dump
    GetRrdDump,

    /// GET_GUEST_CONFIG_PROPERTY (op 11): Get guest config property
    GetGuestConfigProperty { vmid: u32, property: String },

    /// VERIFY_TOKEN (op 12): Verify authentication token
    VerifyToken { token: String },

    /// GET_GUEST_CONFIG_PROPERTIES (op 13): Get multiple guest config properties
    GetGuestConfigProperties { vmid: u32, properties: Vec<String> },
}

impl IpcRequest {
    /// Deserialize an IPC request from message ID and data
    pub fn deserialize(msg_id: i32, data: &[u8]) -> anyhow::Result<Self> {
        let op = CfsIpcOp::try_from(msg_id)
            .map_err(|_| anyhow::anyhow!("Unknown IPC operation code: {msg_id}"))?;

        match op {
            CfsIpcOp::GetFsVersion => Ok(IpcRequest::GetFsVersion),

            CfsIpcOp::GetClusterInfo => Ok(IpcRequest::GetClusterInfo),

            CfsIpcOp::GetGuestList => Ok(IpcRequest::GetGuestList),

            CfsIpcOp::SetStatus => {
                // SET_STATUS: name (256 bytes) + data (rest)
                if data.len() < 256 {
                    anyhow::bail!("SET_STATUS data too short");
                }

                let name = std::ffi::CStr::from_bytes_until_nul(&data[..256])
                    .map_err(|_| anyhow::anyhow!("Invalid name in SET_STATUS"))?
                    .to_str()
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in SET_STATUS name"))?
                    .to_string();

                let status_data = data[256..].to_vec();

                Ok(IpcRequest::SetStatus {
                    name,
                    data: status_data,
                })
            }

            CfsIpcOp::GetStatus => {
                // GET_STATUS: name (256 bytes) + nodename (256 bytes)
                // Matches C struct cfs_status_get_request_header_t (server.c:64-67)
                if data.len() < 512 {
                    anyhow::bail!("GET_STATUS data too short");
                }

                let name = std::ffi::CStr::from_bytes_until_nul(&data[..256])
                    .map_err(|_| anyhow::anyhow!("Invalid name in GET_STATUS"))?
                    .to_str()
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in GET_STATUS name"))?
                    .to_string();

                let node_name = std::ffi::CStr::from_bytes_until_nul(&data[256..512])
                    .map_err(|_| anyhow::anyhow!("Invalid node name in GET_STATUS"))?
                    .to_str()
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in GET_STATUS node name"))?
                    .to_string();

                Ok(IpcRequest::GetStatus { name, node_name })
            }

            CfsIpcOp::GetConfig => {
                // GET_CONFIG: path (null-terminated string)
                let path = std::ffi::CStr::from_bytes_until_nul(data)
                    .map_err(|_| anyhow::anyhow!("Invalid path in GET_CONFIG"))?
                    .to_str()
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in GET_CONFIG path"))?
                    .to_string();

                Ok(IpcRequest::GetConfig { path })
            }

            CfsIpcOp::LogClusterMsg => {
                // LOG_CLUSTER_MSG: priority + ident_len + tag_len + strings
                // C struct (server.c:69-75):
                //   uint8_t priority;
                //   uint8_t ident_len;  // Length INCLUDING null terminator
                //   uint8_t tag_len;    // Length INCLUDING null terminator
                //   char data[];        // ident\0 + tag\0 + message\0
                if data.len() < 3 {
                    anyhow::bail!("LOG_CLUSTER_MSG data too short");
                }

                let priority = data[0];
                let ident_len = data[1] as usize;
                let tag_len = data[2] as usize;

                // Validate lengths (must include null terminator, so >= 1)
                if ident_len < 1 || tag_len < 1 {
                    anyhow::bail!("LOG_CLUSTER_MSG: ident_len or tag_len is 0");
                }

                // Calculate message length (C: datasize - ident_len - tag_len)
                let msg_start = 3 + ident_len + tag_len;
                if data.len() < msg_start + 1 {
                    anyhow::bail!("LOG_CLUSTER_MSG data too short for message");
                }

                // Parse ident (null-terminated C string)
                // C validates: msg[ident_len - 1] == 0
                let ident_data = &data[3..3 + ident_len];
                if ident_data[ident_len - 1] != 0 {
                    anyhow::bail!("LOG_CLUSTER_MSG: ident not null-terminated");
                }
                let ident = std::ffi::CStr::from_bytes_with_nul(ident_data)
                    .map_err(|_| anyhow::anyhow!("Invalid ident in LOG_CLUSTER_MSG"))?
                    .to_str()
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in ident"))?
                    .to_string();

                // Parse tag (null-terminated C string)
                // C validates: msg[ident_len + tag_len - 1] == 0
                let tag_data = &data[3 + ident_len..3 + ident_len + tag_len];
                if tag_data[tag_len - 1] != 0 {
                    anyhow::bail!("LOG_CLUSTER_MSG: tag not null-terminated");
                }
                let tag = std::ffi::CStr::from_bytes_with_nul(tag_data)
                    .map_err(|_| anyhow::anyhow!("Invalid tag in LOG_CLUSTER_MSG"))?
                    .to_str()
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in tag"))?
                    .to_string();

                // Parse message (rest of data, null-terminated)
                // C validates: data[request_size] == 0 (but this is a bug - accesses past buffer)
                // We'll be more lenient and just read until end or first null
                let msg_data = &data[msg_start..];
                let message = std::ffi::CStr::from_bytes_until_nul(msg_data)
                    .map_err(|_| anyhow::anyhow!("Invalid message in LOG_CLUSTER_MSG"))?
                    .to_str()
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in message"))?
                    .to_string();

                Ok(IpcRequest::LogClusterMsg {
                    priority,
                    ident,
                    tag,
                    message,
                })
            }

            CfsIpcOp::GetClusterLog => {
                // GET_CLUSTER_LOG: C struct (server.c:77-83):
                //   uint32_t max_entries;
                //   uint32_t res1, res2, res3;  // reserved, unused
                //   char user[];  // null-terminated user string for filtering
                // Total header: 16 bytes, followed by user string
                const HEADER_SIZE: usize = 16; // 4 u32 fields

                if data.len() <= HEADER_SIZE {
                    // C returns EINVAL if userlen <= 0
                    anyhow::bail!("GET_CLUSTER_LOG: missing user string");
                }

                let max_entries = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
                // Default to 50 if max_entries is 0 (matches C: rh->max_entries ? rh->max_entries : 50)
                let max_entries = if max_entries == 0 { 50 } else { max_entries };

                // Parse user string (null-terminated)
                let user = std::ffi::CStr::from_bytes_until_nul(&data[HEADER_SIZE..])
                    .map_err(|_| anyhow::anyhow!("Invalid user string in GET_CLUSTER_LOG"))?
                    .to_str()
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in user string"))?
                    .to_string();

                Ok(IpcRequest::GetClusterLog { max_entries, user })
            }

            CfsIpcOp::GetRrdDump => Ok(IpcRequest::GetRrdDump),

            CfsIpcOp::GetGuestConfigProperty => {
                // GET_GUEST_CONFIG_PROPERTY: vmid (u32) + property (null-terminated)
                if data.len() < 4 {
                    anyhow::bail!("GET_GUEST_CONFIG_PROPERTY data too short");
                }

                let vmid = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);

                let property = std::ffi::CStr::from_bytes_until_nul(&data[4..])
                    .map_err(|_| anyhow::anyhow!("Invalid property in GET_GUEST_CONFIG_PROPERTY"))?
                    .to_str()
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in property"))?
                    .to_string();

                Ok(IpcRequest::GetGuestConfigProperty { vmid, property })
            }

            CfsIpcOp::VerifyToken => {
                // VERIFY_TOKEN: token (null-terminated string)
                let token = std::ffi::CStr::from_bytes_until_nul(data)
                    .map_err(|_| anyhow::anyhow!("Invalid token in VERIFY_TOKEN"))?
                    .to_str()
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in token"))?
                    .to_string();

                Ok(IpcRequest::VerifyToken { token })
            }

            CfsIpcOp::GetGuestConfigProperties => {
                // GET_GUEST_CONFIG_PROPERTIES: vmid (u32) + num_props (u8) + property list
                if data.len() < 5 {
                    anyhow::bail!("GET_GUEST_CONFIG_PROPERTIES data too short");
                }

                let vmid = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                let num_props = data[4] as usize;

                if num_props == 0 {
                    anyhow::bail!("GET_GUEST_CONFIG_PROPERTIES requires at least one property");
                }

                let mut properties = Vec::with_capacity(num_props);
                let mut remaining = &data[5..];

                for i in 0..num_props {
                    if remaining.is_empty() {
                        anyhow::bail!("Property {i} is missing");
                    }

                    let property = std::ffi::CStr::from_bytes_until_nul(remaining)
                        .map_err(|_| anyhow::anyhow!("Property {i} not null-terminated"))?
                        .to_str()
                        .map_err(|_| anyhow::anyhow!("Property {i} is not valid UTF-8"))?;

                    // Validate property name starts with lowercase letter
                    if property.is_empty() || !property.chars().next().unwrap().is_ascii_lowercase()
                    {
                        anyhow::bail!("Property {i} does not start with [a-z]");
                    }

                    properties.push(property.to_string());
                    remaining = &remaining[property.len() + 1..]; // +1 for null terminator
                }

                // Verify no leftover data
                if !remaining.is_empty() {
                    anyhow::bail!("Leftover data after parsing {num_props} properties");
                }

                Ok(IpcRequest::GetGuestConfigProperties { vmid, properties })
            }
        }
    }
}
