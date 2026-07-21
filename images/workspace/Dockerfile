ARG BASE_IMAGE
ARG GASCAMP_REVISION=f6b248c5926240856dbea83d1d2c5c90ea1c1456

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
RUN install -d -o root -g root -m 0555 /etc/mise
COPY --chmod=0444 images/workspace/etc/mise/config.toml /etc/mise/config.toml
COPY --chmod=0444 .artifacts/expected-tool-versions.json /tmp/expected-tool-versions.json

ENV MISE_DATA_DIR=/opt/gascan/mise
ENV CARGO_HOME=/opt/gascan/mise/cargo
ENV RUSTUP_HOME=/opt/gascan/mise/rustup
ENV MISE_GLOBAL_CONFIG_FILE=/etc/mise/config.toml

RUN mise install --yes erlang@29.0.3 \
    && test "$(mise current erlang)" = "29.0.3" \
    && mise exec erlang@29.0.3 -- erl -noshell -eval 'true = (erlang:system_info(otp_release) =:= "29"), halt().' \
    && mise exec erlang@29.0.3 -- mise install --yes elixir@1.20.2-otp-29 \
    && test "$(mise current elixir)" = "1.20.2-otp-29" \
    && mise install --yes go@1.26.5 java@25.0.2 node@24.18.0 python@3.14.6 ruby@3.4.10 rust@1.97.0 \
    && mise ls --current --installed --json \
       | jq --exit-status --compact-output --sort-keys 'if ((keys|sort) != ["elixir","erlang","go","java","node","python","ruby","rust"]) then error("unexpected mise tool set") else to_entries | map(if ((.value|type)!="array") or ((.value|length)!=1) or (.value[0].installed != true) or (.value[0].active != true) or ((.value[0].version|type)!="string") or (.value[0].version=="") then error("invalid mise ls record") else {key:.key,value:.value[0].version} end) | from_entries end' \
       > /tmp/resolved-tool-versions.json \
    && if ! cmp --silent /tmp/resolved-tool-versions.json /tmp/expected-tool-versions.json; then \
         cat /tmp/resolved-tool-versions.json; \
         cat /tmp/expected-tool-versions.json; \
         exit 1; \
       fi \
    && install -o root -g root -m 0444 /tmp/resolved-tool-versions.json /opt/gascan/image-tool-versions.json \
    && rm -f /tmp/expected-tool-versions.json \
       /tmp/resolved-tool-versions.json

FROM workspace-base AS gascamp-builder

ARG GASCAMP_REVISION
RUN set -eu; \
    test "$GASCAMP_REVISION" = f6b248c5926240856dbea83d1d2c5c90ea1c1456; \
    case "$GASCAMP_REVISION" in *[!0-9a-f]*|'') exit 1;; esac; \
    test "${#GASCAMP_REVISION}" -eq 40; \
    install -d -m 0700 /tmp/gascamp; \
    cd /tmp/gascamp; \
    git init; \
    git remote add origin https://github.com/Liquescent-Development/gascamp.git; \
    git fetch --depth=1 origin "$GASCAMP_REVISION"; \
    git checkout --detach FETCH_HEAD; \
    test "$(git rev-parse HEAD)" = "$GASCAMP_REVISION"; \
    rm -rf .git; \
    mise exec -- cargo test --locked; \
    mise exec -- cargo build --locked --release --bin camp; \
    strip target/release/camp; \
    install -D -o root -g root -m 0555 target/release/camp /out/bin/camp; \
    ln -s camp /out/bin/campd; \
    printf '%s\n' "$GASCAMP_REVISION" >/out/REVISION; \
    chown -R root:root /out; \
    chmod 0444 /out/REVISION; \
    chmod -R a-w /out

FROM workspace-base

ARG GASCAMP_REVISION

COPY .artifacts/playwright-chromium-reviewed /opt/gascan/chromium
COPY --chmod=0440 images/workspace/etc/sudoers.d/workspace /etc/sudoers.d/workspace
COPY --chmod=0555 images/workspace/bin/gascan-entrypoint /usr/local/bin/gascan-entrypoint
COPY --chmod=0555 images/workspace/bin/select-gascamp /usr/local/bin/select-gascamp
COPY --chmod=0555 images/workspace/bin/migrate-workspace-identity /usr/local/bin/migrate-workspace-identity
COPY --chmod=0555 images/workspace/libexec/migrate-workspace-identity-core /usr/local/libexec/gascan/migrate-workspace-identity-core
COPY --from=gascamp-builder /out /opt/gascan/gascamp
COPY --chmod=0444 images/workspace/etc/profile.d/mise.sh /etc/profile.d/mise.sh
COPY --chmod=0444 images/workspace/tests/playwright-smoke.mjs /opt/gascan/tests/playwright-smoke.mjs

USER root
RUN test -x /opt/gascan/gascamp/bin/camp \
    && test -L /opt/gascan/gascamp/bin/campd \
    && test "$(cat /opt/gascan/gascamp/REVISION)" = "$GASCAMP_REVISION" \
    && /usr/local/bin/migrate-workspace-identity \
    && chown workspace:workspace /opt/gascan/mise \
    && install -d -o workspace -g workspace \
       /workspace \
       /home/workspace/.cache \
       /home/workspace/.config/gascan \
    && chmod 0440 /etc/sudoers.d/workspace \
    && visudo -cf /etc/sudoers.d/workspace \
    && chmod 0555 /opt/gascan/chromium/chrome-linux/chrome \
    && chmod 0555 /opt/gascan/chromium/chrome-linux/chrome-wrapper \
    && chmod 0555 /opt/gascan/chromium/chrome-linux/chrome_crashpad_handler \
    && chmod 0555 /opt/gascan/chromium/chrome-linux/chrome_sandbox \
    && chmod 0555 /opt/gascan/chromium/chrome-linux/libEGL.so \
    && chmod 0555 /opt/gascan/chromium/chrome-linux/libGLESv2.so \
    && chmod 0555 /opt/gascan/chromium/chrome-linux/libvulkan.so.1 \
    && chmod 0555 /opt/gascan/chromium/chrome-linux/libvk_swiftshader.so \
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
