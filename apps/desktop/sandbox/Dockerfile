# Lathe Cloud sandbox image.
# Build with: pnpm build:sandbox
FROM rust:1.99-bookworm AS agent-build
WORKDIR /build
COPY packages/agent ./agent
RUN cargo build --release --manifest-path agent/Cargo.toml --locked

FROM ubuntu:24.04

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git gnupg openssh-client software-properties-common \
    unzip wget \
  && mkdir -p /etc/apt/keyrings \
  && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
    | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
  && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_24.x nodistro main" \
    > /etc/apt/sources.list.d/nodesource.list \
  && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
    -o /etc/apt/keyrings/githubcli.gpg \
  && chmod go+r /etc/apt/keyrings/githubcli.gpg \
  && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli.gpg] https://cli.github.com/packages stable main" \
    > /etc/apt/sources.list.d/github-cli.list \
  && wget -qO- https://packages.adoptium.net/artifactory/api/gpg/key/public \
    | gpg --dearmor -o /etc/apt/keyrings/adoptium.gpg \
  && echo "deb [signed-by=/etc/apt/keyrings/adoptium.gpg] https://packages.adoptium.net/artifactory/deb $(. /etc/os-release && echo \"$VERSION_CODENAME\") main" \
    > /etc/apt/sources.list.d/adoptium.list \
  && apt-get update \
  && apt-get install -y --no-install-recommends nodejs gh temurin-21-jdk temurin-25-jdk \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*

# Java 21 is the default while both supported runtimes remain available.
RUN ln -s /usr/lib/jvm/temurin-21-jdk-* /usr/lib/jvm/default
ENV JAVA_HOME=/usr/lib/jvm/default
ENV PATH=/usr/lib/jvm/default/bin:${PATH}
ENV NODE_PATH=/usr/lib/node_modules

COPY --from=agent-build /build/agent/target/release/lathe-agent /usr/local/bin/lathe-agent

# Ubuntu images ship a default `ubuntu` user at UID 1000, which would collide
# with the agent user below; drop it first so fresh pulls of the base image
# build identically on every host.
RUN userdel -r ubuntu \
  && useradd --create-home --uid 1000 --shell /bin/bash agent \
  && mkdir -p /home/agent/workspace \
  && chown -R agent:agent /home/agent

COPY apps/desktop/sandbox/sandbox-entrypoint.sh /usr/local/bin/sandbox-entrypoint
# Git for Windows checks shell scripts out with CRLF line endings when
# core.autocrlf is set, and the container cannot execute a CRLF shebang. Strip
# carriage returns defensively so the image runs regardless of host checkout
# settings (fresh clones stay LF via .gitattributes).
RUN sed -i 's/\r$//' /usr/local/bin/sandbox-entrypoint \
  && chmod 0755 /usr/local/bin/sandbox-entrypoint

USER agent
WORKDIR /home/agent
ENTRYPOINT ["/usr/local/bin/sandbox-entrypoint"]
CMD ["bash"]
