ARG BASE_IMAGE
FROM ${BASE_IMAGE}

ARG UBUNTU_SNAPSHOT

COPY .artifacts/mise-linux-arm64 /opt/gascan/artifacts/mise-linux-arm64
COPY .artifacts/playwright-chromium-linux-arm64.zip /opt/gascan/artifacts/playwright-chromium-linux-arm64.zip
COPY --chmod=0440 images/workspace/etc/sudoers.d/workspace /etc/sudoers.d/workspace
COPY --chmod=0555 images/workspace/bin/gascan-entrypoint /usr/local/bin/gascan-entrypoint
COPY --chmod=0444 images/workspace/etc/mise/config.toml /etc/mise/config.toml
COPY --chmod=0444 images/workspace/etc/profile.d/mise.sh /etc/profile.d/mise.sh
COPY --chmod=0444 images/workspace/tests/playwright-smoke.mjs /opt/gascan/tests/playwright-smoke.mjs
COPY --chmod=0444 tests/image/system-tools.txt /tmp/gascan-system-tools.txt

RUN snapshot="$(printf '%s' "${UBUNTU_SNAPSHOT}" | tr -d -- '-:')" \
    && printf '%s\n' \
       "Types: deb" \
       "URIs: https://snapshot.ubuntu.com/ubuntu/${snapshot}/" \
       "Suites: noble noble-updates noble-security" \
       "Components: main universe" \
       "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg" \
       > /etc/apt/sources.list.d/ubuntu.sources \
    && DEBIAN_FRONTEND=noninteractive apt-get update \
    && DEBIAN_FRONTEND=noninteractive xargs --arg-file=/tmp/gascan-system-tools.txt apt-get install --yes --no-install-recommends \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 workspace \
    && useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash workspace \
    && install -d -o workspace -g workspace \
       /workspace \
       /opt/gascan/mise \
       /home/workspace/.cache \
       /home/workspace/.config/gascan \
       /opt/gascan/chromium \
       /opt/gascan/tests \
    && chmod 0440 /etc/sudoers.d/workspace \
    && visudo -cf /etc/sudoers.d/workspace \
    && install -m 0555 /opt/gascan/artifacts/mise-linux-arm64 /usr/local/bin/mise \
    && unzip -q /opt/gascan/artifacts/playwright-chromium-linux-arm64.zip -d /opt/gascan/chromium \
    && chmod 0444 /opt/gascan/artifacts/playwright-chromium-linux-arm64.zip \
    && rm -f /tmp/gascan-system-tools.txt

ENV HOME=/home/workspace
ENV MISE_DATA_DIR=/opt/gascan/mise
ENV MISE_CACHE_DIR=/home/workspace/.cache/mise
ENV MISE_GLOBAL_CONFIG_FILE=/etc/mise/config.toml
ENV PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/opt/gascan/chromium/chrome-linux/chrome
ENV PATH=/opt/gascan/mise/shims:${PATH}
USER workspace:workspace
RUN mise install --yes \
    && printf '{\n  "elixir": "%s",\n  "go": "%s",\n  "java": "%s",\n  "node": "%s",\n  "python": "%s",\n  "ruby": "%s",\n  "rust": "%s"\n}\n' \
       "$(mise current elixir)" \
       "$(mise current go)" \
       "$(mise current java)" \
       "$(mise current node)" \
       "$(mise current python)" \
       "$(mise current ruby)" \
       "$(mise current rust)" \
       > /opt/gascan/image-tool-versions.json
WORKDIR /workspace
VOLUME ["/opt/gascan/mise", "/home/workspace/.cache", "/home/workspace/.config/gascan"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/gascan-entrypoint"]
