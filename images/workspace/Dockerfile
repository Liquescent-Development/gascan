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
COPY --chmod=0555 images/workspace/bin/start-gascan-sshd /usr/local/bin/start-gascan-sshd
COPY --chmod=0555 images/workspace/bin/select-gascamp /usr/local/bin/select-gascamp
COPY --chmod=0555 images/workspace/bin/migrate-workspace-identity /usr/local/bin/migrate-workspace-identity
COPY --chmod=0555 images/workspace/libexec/migrate-workspace-identity-core /usr/local/libexec/gascan/migrate-workspace-identity-core
COPY --from=gascamp-builder /out /opt/gascan/gascamp
COPY --chmod=0444 images/workspace/etc/profile.d/mise.sh /etc/profile.d/mise.sh
COPY --chmod=0444 images/workspace/tests/playwright-smoke.mjs /opt/gascan/tests/playwright-smoke.mjs
COPY --chmod=0555 images/workspace/tests/workstation-contract.sh /opt/gascan/tests/workstation-contract.sh
COPY --chmod=0555 images/workspace/tests/ssh-contract.sh /opt/gascan/tests/ssh-contract.sh
COPY --chmod=0555 images/workspace/bin/install-workstation-artifacts /usr/local/bin/install-workstation-artifacts
COPY --chmod=0555 images/workspace/bin/run-workstation-step /usr/local/bin/run-workstation-step
COPY --chmod=0555 images/workspace/bin/configure-workstation-home /usr/local/bin/configure-workstation-home
COPY --chmod=0555 images/workspace/bin/configure-shell-home /usr/local/bin/configure-shell-home
COPY --chmod=0555 images/workspace/bin/initialize-rust-home /usr/local/bin/initialize-rust-home
RUN install -d -o root -g root -m 0555 /etc/gascan
COPY --chmod=0444 images/workspace/etc/gascan/bashrc /etc/gascan/bashrc
COPY --chmod=0444 images/workspace/etc/gascan/starship.toml /opt/gascan/shell/presets/starship.toml
COPY --chmod=0444 images/workspace/etc/gascan/starship-nerd-font.toml /opt/gascan/shell/presets/starship-nerd-font.toml
COPY --chmod=0444 images/workspace/versions.lock /tmp/versions.lock
COPY workstation /tmp/workstation

USER root
RUN --network=none test -x /opt/gascan/gascamp/bin/camp \
    && test -L /opt/gascan/gascamp/bin/campd \
    && test "$(cat /opt/gascan/gascamp/REVISION)" = "$GASCAMP_REVISION" \
    && /usr/local/bin/migrate-workspace-identity \
    && chown -R root:root /opt/gascan/mise \
    && chmod -R a-w /opt/gascan/mise \
    && /usr/local/bin/run-workstation-step test-directory-mode \
       chmod 0555 /opt/gascan/tests \
    && PATH="/opt/gascan/mise/shims:$PATH" /usr/local/bin/run-workstation-step installer \
       /usr/local/bin/install-workstation-artifacts \
       install /tmp/workstation /opt/gascan/workstation /usr/local/bin /tmp/versions.lock \
       /tmp/workstation/target-lock.toml \
    && printf 'workstation assembly: installer complete\n' >&2 \
    && /usr/local/bin/run-workstation-step shell-layout \
       sh -c 'install -d -o root -g root -m 0555 /opt/gascan/shell/bin; \
       test -x /opt/gascan/workstation/bin/starship; \
       ln -s ../../workstation/bin/starship /opt/gascan/shell/bin/starship; \
       chown -R root:root /opt/gascan/shell; \
       chown -h root:root /opt/gascan/shell/bin/starship; \
       chmod 0555 /opt/gascan/shell /opt/gascan/shell/bin /opt/gascan/shell/presets; \
       chmod 0444 /opt/gascan/shell/presets/starship.toml \
         /opt/gascan/shell/presets/starship-nerd-font.toml; \
       test "$(readlink /opt/gascan/shell/bin/starship)" = ../../workstation/bin/starship' \
    && /usr/local/bin/run-workstation-step immutable-owner \
       chown -R root:root /opt/gascan/workstation \
    && /usr/local/bin/run-workstation-step immutable-mode \
       sh -c 'test -z "$(find /opt/gascan/workstation ! -type l \( -perm -020 -o -perm -002 \) -print -quit)"' \
    && /usr/local/bin/run-workstation-step immutable-ownership \
       sh -c 'test -z "$(find /opt/gascan/workstation \( ! -user root -o ! -group root \) -print -quit)"' \
    && /usr/local/bin/run-workstation-step home-directories \
       sh -c 'install -d -o workspace -g workspace /workspace; \
       install -d -o workspace -g workspace -m 0700 \
       /home/workspace/.local /home/workspace/.cache; \
       install -d -o root -g workspace -m 1770 /home/workspace/.config' \
    && /usr/local/bin/run-workstation-step home-configuration \
       sudo -n -u workspace env HOME=/home/workspace /usr/local/bin/configure-workstation-home \
    && /usr/local/bin/run-workstation-step shell-startup \
       sh -c 'hook_line=". /etc/gascan/bashrc"; \
       test -f /etc/gascan/bashrc && test ! -L /etc/gascan/bashrc; \
       test "$(stat -c %U:%G:%a /etc/gascan/bashrc)" = root:root:444; \
       for startup in /home/workspace/.bashrc /root/.bashrc; do \
         test -f "$startup" && test ! -L "$startup"; \
         case "$startup" in \
           /home/workspace/.bashrc) expected=workspace:workspace ;; \
           /root/.bashrc) expected=root:root ;; \
           *) exit 1 ;; \
         esac; \
         test "$(stat -c %U:%G "$startup")" = "$expected"; \
         printf "\n%s\n" "$hook_line" >>"$startup"; \
         test "$(tail -n 1 "$startup")" = "$hook_line"; \
       done' \
    && /usr/local/bin/run-workstation-step gascan-config-boundary \
       sh -c 'chown root:workspace /home/workspace/.config/gascan; \
       chmod 1770 /home/workspace/.config/gascan; \
       sudo -n -u workspace install -d -m 0700 /home/workspace/.config/.workspace-probe; \
       test -d /home/workspace/.config/.workspace-probe; \
       if sudo -n -u workspace mv /home/workspace/.config/gascan \
         /home/workspace/.config/gascan.moved 2>/dev/null; then exit 1; fi; \
       rm -rf /home/workspace/.config/.workspace-probe' \
    && /usr/local/bin/run-workstation-step home-link-owner \
       chown -h workspace:workspace \
       /home/workspace/.claude \
       /home/workspace/.codex \
       /home/workspace/.pi \
       /home/workspace/.config/herdr \
    && /usr/local/bin/run-workstation-step sudoers-mode \
       chmod 0440 /etc/sudoers.d/workspace \
    && /usr/local/bin/run-workstation-step sudoers-validation \
       visudo -cf /etc/sudoers.d/workspace \
    && /usr/local/bin/run-workstation-step chromium-mode chmod 0555 \
       /opt/gascan/chromium/chrome-linux/chrome \
       /opt/gascan/chromium/chrome-linux/chrome-wrapper \
       /opt/gascan/chromium/chrome-linux/chrome_crashpad_handler \
       /opt/gascan/chromium/chrome-linux/chrome_sandbox \
       /opt/gascan/chromium/chrome-linux/libEGL.so \
       /opt/gascan/chromium/chrome-linux/libGLESv2.so \
       /opt/gascan/chromium/chrome-linux/libvulkan.so.1 \
       /opt/gascan/chromium/chrome-linux/libvk_swiftshader.so \
    && /usr/local/bin/run-workstation-step chromium-command \
       test -x /opt/gascan/chromium/chrome-linux/chrome \
    && /usr/local/bin/run-workstation-step chromium-seal \
       chmod -R a-w /opt/gascan/chromium \
    && /usr/local/bin/run-workstation-step temporary-cleanup \
       sh -c 'rm -rf /tmp/workstation /tmp/versions.lock; rm -f /usr/local/bin/run-workstation-step'

ENV HOME=/home/workspace
ENV SHELL=/bin/bash
ENV XDG_DATA_HOME=/home/workspace/.local/share
ENV XDG_CACHE_HOME=/home/workspace/.cache
ENV XDG_CONFIG_HOME=/home/workspace/.config
ENV MISE_DATA_DIR=/home/workspace/.local/share/mise
ENV MISE_CACHE_DIR=/home/workspace/.cache/mise
ENV MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml
ENV MISE_SYSTEM_CONFIG_FILE=/etc/mise/config.toml
ENV MISE_STATE_DIR=/home/workspace/.config/gascan/mise-state
ENV MISE_SYSTEM_DATA_DIR=/opt/gascan/mise
ENV CARGO_HOME=/home/workspace/.local/share/cargo
ENV MISE_CARGO_HOME=/home/workspace/.local/share/cargo
ENV RUSTUP_HOME=/home/workspace/.local/share/rustup
ENV MISE_RUSTUP_HOME=/home/workspace/.local/share/rustup
ENV NPM_CONFIG_PREFIX=/home/workspace/.local
ENV NPM_CONFIG_CACHE=/home/workspace/.cache/npm
ENV GOPATH=/home/workspace/.local/share/go
ENV GOBIN=/home/workspace/.local/bin
ENV GOCACHE=/home/workspace/.cache/go-build
ENV GOMODCACHE=/home/workspace/.cache/go-mod
ENV PYTHONUSERBASE=/home/workspace/.local
ENV GEM_HOME=/home/workspace/.local/share/gem
ENV MIX_HOME=/home/workspace/.local/share/mix
ENV HEX_HOME=/home/workspace/.local/share/hex
ENV REBAR_CACHE_DIR=/home/workspace/.cache/rebar3
ENV PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/opt/gascan/chromium/chrome-linux/chrome
ENV CLAUDE_CONFIG_DIR=/home/workspace/.config/gascan/agents/claude
ENV CODEX_HOME=/home/workspace/.config/gascan/agents/codex
ENV PI_CODING_AGENT_DIR=/home/workspace/.config/gascan/agents/pi
ENV PI_CODING_AGENT_SESSION_DIR=/home/workspace/.cache/pi
ENV HERDR_CONFIG_PATH=/home/workspace/.config/gascan/herdr/config.toml
ENV GH_CONFIG_DIR=/home/workspace/.config/gascan/gh
ENV GLAB_CONFIG_DIR=/home/workspace/.config/gascan/glab
ENV PATH=/home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin
USER workspace:workspace
WORKDIR /workspace
VOLUME ["/home/workspace/.local", "/home/workspace/.cache", "/home/workspace/.config"]
ENTRYPOINT ["/usr/local/bin/gascan-entrypoint"]
