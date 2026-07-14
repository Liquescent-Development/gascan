ARG BASE_IMAGE
ARG UBUNTU_SNAPSHOT

FROM ${BASE_IMAGE} AS gascamp-builder

ARG UBUNTU_SNAPSHOT
ARG GASCAMP_REVISION=f6b248c5926240856dbea83d1d2c5c90ea1c1456

COPY .artifacts/mise-linux-arm64 /usr/local/bin/mise
COPY --chmod=0444 images/workspace/etc/mise/config.toml /etc/mise/config.toml

RUN snapshot="$(printf '%s' "${UBUNTU_SNAPSHOT}" | tr -d -- '-:')" \
    && printf '%s\n' \
       "Types: deb" \
       "URIs: https://snapshot.ubuntu.com/ubuntu/${snapshot}/" \
       "Suites: noble noble-updates noble-security" \
       "Components: main universe" \
       "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg" \
       > /etc/apt/sources.list.d/ubuntu.sources \
    && DEBIAN_FRONTEND=noninteractive apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
       build-essential ca-certificates git libssl-dev pkg-config \
    && rm -rf /var/lib/apt/lists/* \
    && chmod 0555 /usr/local/bin/mise \
    && MISE_DATA_DIR=/opt/gascan/mise MISE_GLOBAL_CONFIG_FILE=/etc/mise/config.toml mise install --yes rust \
    && git init /tmp/gascamp \
    && git -C /tmp/gascamp remote add origin https://github.com/gascan/gascamp.git \
    && git -C /tmp/gascamp fetch --depth 1 origin "${GASCAMP_REVISION}" \
    && test "$(git -C /tmp/gascamp rev-parse FETCH_HEAD)" = "${GASCAMP_REVISION}" \
    && git -C /tmp/gascamp checkout --detach "${GASCAMP_REVISION}" \
    && cd /tmp/gascamp \
    && /opt/gascan/mise/installs/rust/1.97.0/bin/cargo test --locked \
    && /opt/gascan/mise/installs/rust/1.97.0/bin/cargo build --locked --release --bin camp \
    && strip target/release/camp \
    && install -D -m 0555 target/release/camp /out/bin/camp \
    && ln -s camp /out/bin/campd \
    && printf '%s\n' "${GASCAMP_REVISION}" > /out/REVISION \
    && chmod -R a-w /out

FROM ${BASE_IMAGE}

ARG UBUNTU_SNAPSHOT

COPY .artifacts/mise-linux-arm64 /opt/gascan/artifacts/mise-linux-arm64
COPY --chmod=0444 .artifacts/expected-tool-versions.json /opt/gascan/artifacts/expected-tool-versions.json
COPY .artifacts/playwright-chromium-reviewed/chrome-linux /opt/gascan/chromium/chrome-linux
COPY --chmod=0440 images/workspace/etc/sudoers.d/workspace /etc/sudoers.d/workspace
COPY --chmod=0555 images/workspace/bin/gascan-entrypoint /usr/local/bin/gascan-entrypoint
COPY --chmod=0555 images/workspace/bin/select-gascamp /usr/local/bin/select-gascamp
COPY --from=gascamp-builder /out /opt/gascan/gascamp
COPY --chmod=0444 images/workspace/etc/mise/config.toml /etc/mise/config.toml
COPY --chmod=0444 images/workspace/etc/profile.d/mise.sh /etc/profile.d/mise.sh
COPY --chmod=0444 images/workspace/tests/playwright-smoke.mjs /opt/gascan/tests/playwright-smoke.mjs
COPY --chmod=0444 tests/image/system-tools.txt /tmp/gascan-system-tools.txt

RUN test -x /opt/gascan/gascamp/bin/camp \
    && test -L /opt/gascan/gascamp/bin/campd \
    && test "$(cat /opt/gascan/gascamp/REVISION)" = f6b248c5926240856dbea83d1d2c5c90ea1c1456

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
    && chmod 0440 /etc/sudoers.d/workspace \
    && visudo -cf /etc/sudoers.d/workspace \
    && install -m 0555 /opt/gascan/artifacts/mise-linux-arm64 /usr/local/bin/mise \
    && test -x /opt/gascan/chromium/chrome-linux/chrome \
    && chmod -R a-w /opt/gascan/chromium \
    && rm -f /tmp/gascan-system-tools.txt

ENV HOME=/home/workspace
ENV MISE_DATA_DIR=/opt/gascan/mise
ENV MISE_CACHE_DIR=/home/workspace/.cache/mise
ENV MISE_GLOBAL_CONFIG_FILE=/etc/mise/config.toml
ENV PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/opt/gascan/chromium/chrome-linux/chrome
ENV PATH=/opt/gascan/mise/shims:${PATH}
USER workspace:workspace
RUN mise install --yes \
    && install -d /home/workspace/.cache/gascan-image \
    && jq --null-input \
       --arg elixir "$(mise current elixir)" \
       --arg go "$(mise current go)" \
       --arg java "$(mise current java)" \
       --arg node "$(mise current node)" \
       --arg python "$(mise current python)" \
       --arg ruby "$(mise current ruby)" \
       --arg rust "$(mise current rust)" \
       '$ARGS.named' > /home/workspace/.cache/gascan-image/resolved-tool-versions.json
USER root
RUN jq --exit-status --slurpfile expected /opt/gascan/artifacts/expected-tool-versions.json \
       '. == $expected[0]' \
       /home/workspace/.cache/gascan-image/resolved-tool-versions.json >/dev/null \
    && install -o root -g root -m 0444 \
       /home/workspace/.cache/gascan-image/resolved-tool-versions.json \
       /opt/gascan/image-tool-versions.json \
    && rm -f /home/workspace/.cache/gascan-image/resolved-tool-versions.json \
    && rmdir /home/workspace/.cache/gascan-image
USER workspace:workspace
WORKDIR /workspace
VOLUME ["/opt/gascan/mise", "/home/workspace/.cache", "/home/workspace/.config/gascan"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/gascan-entrypoint"]
