ARG BASE_IMAGE

FROM ${BASE_IMAGE} AS workspace-base

COPY --chmod=0444 tests/image/system-tools.txt /tmp/system-tools.txt
RUN test "$(LC_ALL=C sort -u /tmp/system-tools.txt | wc -l | tr -d ' ')" = "$(wc -l < /tmp/system-tools.txt | tr -d ' ')" \
    && apt-get -o Acquire::Retries=0 update \
    && DEBIAN_FRONTEND=noninteractive xargs apt-get \
         -o Acquire::Retries=0 install --yes --no-install-recommends </tmp/system-tools.txt \
    && while IFS= read -r package; do dpkg-query -W -f='${db:Status-Status}\n' "$package" | grep -Fx installed; done </tmp/system-tools.txt \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* /tmp/system-tools.txt

COPY --chmod=0555 .artifacts/mise-linux-arm64 /usr/local/bin/mise
COPY --chmod=0444 images/workspace/etc/mise/config.toml /etc/mise/config.toml
COPY --chmod=0444 .artifacts/expected-tool-versions.json /tmp/expected-tool-versions.json

ENV MISE_DATA_DIR=/opt/gascan/mise
ENV MISE_GLOBAL_CONFIG_FILE=/etc/mise/config.toml

RUN mise install --yes \
    && mise current --json \
       | jq --compact-output --sort-keys 'with_entries(.value = .value.version)' \
       > /tmp/resolved-tool-versions.json \
    && cmp /tmp/resolved-tool-versions.json /tmp/expected-tool-versions.json \
    && install -o root -g root -m 0444 /tmp/resolved-tool-versions.json /opt/gascan/image-tool-versions.json \
    && rm -f /tmp/expected-tool-versions.json \
       /tmp/resolved-tool-versions.json

FROM workspace-base AS gascamp-builder

COPY bundles/gascamp_source_vendor/tree/source/ /tmp/gascamp/
COPY bundles/gascamp_source_vendor/tree/vendor/ /tmp/gascamp/vendor/
COPY --chmod=0444 bundles/gascamp_source_vendor/tree/.cargo/config.toml /tmp/gascamp/.cargo/config.toml

RUN cd /tmp/gascamp \
    && /opt/gascan/mise/installs/rust/1.97.0/bin/cargo test --locked --offline --frozen \
    && /opt/gascan/mise/installs/rust/1.97.0/bin/cargo build --locked --offline --frozen --release --bin camp \
    && strip target/release/camp \
    && install -D -m 0555 target/release/camp /out/bin/camp \
    && ln -s camp /out/bin/campd \
    && printf '%s\n' f6b248c5926240856dbea83d1d2c5c90ea1c1456 > /out/REVISION \
    && chmod -R a-w /out

FROM workspace-base

COPY .artifacts/playwright-chromium-reviewed/chrome-linux /opt/gascan/chromium/chrome-linux
COPY --chmod=0440 images/workspace/etc/sudoers.d/workspace /etc/sudoers.d/workspace
COPY --chmod=0555 images/workspace/bin/gascan-entrypoint /usr/local/bin/gascan-entrypoint
COPY --chmod=0555 images/workspace/bin/select-gascamp /usr/local/bin/select-gascamp
COPY --from=gascamp-builder /out /opt/gascan/gascamp
COPY --chmod=0444 images/workspace/etc/profile.d/mise.sh /etc/profile.d/mise.sh
COPY --chmod=0444 images/workspace/tests/playwright-smoke.mjs /opt/gascan/tests/playwright-smoke.mjs

USER root
RUN test -x /opt/gascan/gascamp/bin/camp \
    && test -L /opt/gascan/gascamp/bin/campd \
    && test "$(cat /opt/gascan/gascamp/REVISION)" = f6b248c5926240856dbea83d1d2c5c90ea1c1456 \
    && groupadd --gid 1000 workspace \
    && useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash workspace \
    && chown workspace:workspace /opt/gascan/mise \
    && install -d -o workspace -g workspace \
       /workspace \
       /home/workspace/.cache \
       /home/workspace/.config/gascan \
    && chmod 0440 /etc/sudoers.d/workspace \
    && visudo -cf /etc/sudoers.d/workspace \
    && test -x /opt/gascan/chromium/chrome-linux/chrome \
    && chmod -R a-w /opt/gascan/chromium

ENV HOME=/home/workspace
ENV MISE_CACHE_DIR=/home/workspace/.cache/mise
ENV PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/opt/gascan/chromium/chrome-linux/chrome
ENV PATH=/opt/gascan/mise/shims:${PATH}
USER workspace:workspace
WORKDIR /workspace
VOLUME ["/opt/gascan/mise", "/home/workspace/.cache", "/home/workspace/.config/gascan"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/gascan-entrypoint"]
