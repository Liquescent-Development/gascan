ARG BASE_IMAGE

FROM ${BASE_IMAGE} AS offline-base

COPY bundles/ubuntu_packages/repository /opt/gascan/offline/ubuntu/repository
COPY --chmod=0444 bundles/ubuntu_packages/package-manifest.tsv /opt/gascan/offline/ubuntu/package-manifest.tsv

RUN rm -f /etc/apt/sources.list /etc/apt/sources.list.d/* \
    && dpkg-query --show --showformat='${Package}\t${Version}\t${Architecture}\n' \
       | LC_ALL=C sort > /tmp/gascan-packages.before \
    && find /opt/gascan/offline/ubuntu/repository -type f -name '*.deb' -print0 \
       | LC_ALL=C sort -z > /tmp/gascan-packages.list \
    && test -s /tmp/gascan-packages.list \
    && DEBIAN_FRONTEND=noninteractive xargs -0 --arg-file=/tmp/gascan-packages.list apt-get \
           -o Dir::Etc::sourcelist=/dev/null \
           -o Dir::Etc::sourceparts=- \
           -o Dir::Bin::methods=/nonexistent \
           -o Acquire::Retries=0 \
           install --yes --no-install-recommends \
    && while IFS="$(printf '\t')" read -r package version architecture _; do \
           test "$(dpkg-query --show --showformat='${Version}\t${Architecture}' "$package")" = "$version$(printf '\t')$architecture"; \
       done < /opt/gascan/offline/ubuntu/package-manifest.tsv \
    && cut -f1-3 /opt/gascan/offline/ubuntu/package-manifest.tsv \
       | LC_ALL=C sort > /tmp/gascan-packages.locked \
    && dpkg-query --show --showformat='${Package}\t${Version}\t${Architecture}\n' \
       | LC_ALL=C sort > /tmp/gascan-packages.after \
    && cut -f1 /tmp/gascan-packages.before | LC_ALL=C sort -u > /tmp/gascan-packages.before-names \
    && cut -f1 /tmp/gascan-packages.after | LC_ALL=C sort -u > /tmp/gascan-packages.after-names \
    && test -z "$(comm -23 /tmp/gascan-packages.before-names /tmp/gascan-packages.after-names)" \
    && test -z "$(comm -13 /tmp/gascan-packages.before /tmp/gascan-packages.after \
       | comm -23 - /tmp/gascan-packages.locked)" \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* /opt/gascan/offline/ubuntu \
       /tmp/gascan-packages.list /tmp/gascan-packages.before \
       /tmp/gascan-packages.after /tmp/gascan-packages.locked \
       /tmp/gascan-packages.before-names /tmp/gascan-packages.after-names

COPY --chmod=0555 .artifacts/mise-linux-arm64 /usr/local/bin/mise
COPY --chmod=0444 images/workspace/etc/mise/config.toml /etc/mise/config.toml
COPY bundles/mise_runtimes/mise-runtimes-linux-arm64.tar.zst /tmp/mise-runtimes-linux-arm64.tar.zst
COPY --chmod=0444 bundles/mise_runtimes/mise-runtimes-linux-arm64.manifest.tsv /tmp/mise-runtimes-linux-arm64.manifest.tsv
COPY --chmod=0444 bundles/mise_runtimes/mise-current.json /tmp/mise-current.json
COPY --chmod=0444 .artifacts/expected-tool-versions.json /tmp/expected-tool-versions.json

ENV MISE_DATA_DIR=/opt/gascan/mise
ENV MISE_GLOBAL_CONFIG_FILE=/etc/mise/config.toml

RUN test ! -e /opt/gascan/mise \
    && tar --zstd --list --file /tmp/mise-runtimes-linux-arm64.tar.zst \
       | sed 's:/$::' | LC_ALL=C sort > /tmp/mise-archive-paths \
    && cut -f1 /tmp/mise-runtimes-linux-arm64.manifest.tsv \
       | LC_ALL=C sort > /tmp/mise-manifest-paths \
    && cmp /tmp/mise-archive-paths /tmp/mise-manifest-paths \
    && tar --zstd --extract --file /tmp/mise-runtimes-linux-arm64.tar.zst --directory / \
    && tar --zstd --compare --file /tmp/mise-runtimes-linux-arm64.tar.zst --directory / \
    && mise current --json \
       | jq --compact-output --sort-keys 'with_entries(.value = .value.version)' \
       > /tmp/resolved-tool-versions.json \
    && jq --compact-output --sort-keys 'with_entries(.value = .value.version)' /tmp/mise-current.json \
       > /tmp/bundle-tool-versions.json \
    && cmp /tmp/resolved-tool-versions.json /tmp/bundle-tool-versions.json \
    && cmp /tmp/resolved-tool-versions.json /tmp/expected-tool-versions.json \
    && install -o root -g root -m 0444 /tmp/resolved-tool-versions.json /opt/gascan/image-tool-versions.json \
    && rm -f /tmp/mise-runtimes-linux-arm64.tar.zst \
       /tmp/mise-current.json \
       /tmp/mise-archive-paths \
       /tmp/mise-manifest-paths \
       /tmp/mise-runtimes-linux-arm64.manifest.tsv \
       /tmp/bundle-tool-versions.json \
       /tmp/expected-tool-versions.json \
       /tmp/resolved-tool-versions.json

FROM offline-base AS gascamp-builder

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

FROM offline-base

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
