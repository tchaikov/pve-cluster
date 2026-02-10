FROM debian:stable

# Disable proxy for apt
RUN echo 'Acquire::http::Proxy "false";' > /etc/apt/apt.conf.d/99noproxy

# Always use host's apt sources for consistent package installation
# Copy from host /etc/apt/sources.list.d/debian.sources if it exists
COPY debian.sources /etc/apt/sources.list.d/debian.sources

# Copy Proxmox keyring and repository configuration
RUN mkdir -p /usr/share/keyrings
COPY proxmox-archive-keyring.gpg /usr/share/keyrings/
COPY pve-no-subscription.sources /etc/apt/sources.list.d/

# Install runtime dependencies
# For Rust pmxcfs, C pmxcfs, and mixed cluster testing
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    # Rust pmxcfs dependencies
    libfuse3-4 \
    fuse3 \
    # C pmxcfs dependencies (for mixed cluster testing)
    libfuse2 \
    libglib2.0-0 \
    # Shared dependencies
    libsqlite3-0 \
    libqb100 \
    librrd8t64 \
    rrdtool \
    rrdcached \
    libcorosync-common4 \
    libcpg4 \
    libquorum5 \
    libcmap4 \
    libvotequorum8 \
    libcfg7 \
    socat \
    procps \
    corosync \
    corosync-qdevice \
    iputils-ping \
    iproute2 \
    sqlite3 \
    bc \
    # Testing utilities
    jq \
    file \
    uuid-runtime \
    # Perl and testing dependencies for C tests
    perl \
    libtest-simple-perl \
    libtest-mockmodule-perl \
    libjson-perl \
    libdevel-cycle-perl \
    libclone-perl \
    libnet-ssleay-perl \
    libnet-ip-perl \
    && rm -rf /var/lib/apt/lists/*

# Install Proxmox PVE packages for C tests
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    libpve-cluster-perl \
    libpve-common-perl \
    pve-cluster \
    && rm -rf /var/lib/apt/lists/*

# Create test directories
RUN mkdir -p /test/db \
    /test/run \
    /test/pve \
    /test/etc/corosync \
    /etc/corosync \
    /etc/pve \
    /var/lib/pve-cluster \
    /var/lib/rrdcached/db \
    /run/pmxcfs \
    /var/log/corosync

# Create FUSE config
RUN echo "user_allow_other" > /etc/fuse.conf

# Note: Test files and PVE modules are available via /workspace volume mount at runtime
# - Test files: /workspace/src/test/
# - PVE modules: /workspace/src/PVE/
# - Compiled binary: /workspace/src/pmxcfs-rs/target/release/pmxcfs

# Working directory
WORKDIR /test

# Note: Health check and scripts access files via /workspace mount
# Health check (verifies pmxcfs is running and FUSE is mounted)
# Reduced start_period from 15s to 5s since we eliminated the restart
HEALTHCHECK --interval=5s --timeout=3s --start-period=5s --retries=3 \
    CMD /workspace/src/pmxcfs-rs/integration-tests/docker/healthcheck.sh

# Default command (can be overridden by docker-compose)
CMD ["/workspace/src/pmxcfs-rs/integration-tests/docker/start-cluster-node.sh"]
