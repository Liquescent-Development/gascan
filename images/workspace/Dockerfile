ARG BASE_IMAGE
FROM ${BASE_IMAGE}

ARG UBUNTU_SNAPSHOT

COPY .artifacts/mise-linux-arm64 /opt/gascan/artifacts/mise-linux-arm64
COPY .artifacts/playwright-chromium-linux-arm64.zip /opt/gascan/artifacts/playwright-chromium-linux-arm64.zip
COPY --chmod=0440 images/workspace/etc/sudoers.d/workspace /etc/sudoers.d/workspace
COPY --chmod=0555 images/workspace/bin/gascan-entrypoint /usr/local/bin/gascan-entrypoint

RUN snapshot="$(printf '%s' "${UBUNTU_SNAPSHOT}" | tr -d -- '-:')" \
    && printf '%s\n' \
       "Types: deb" \
       "URIs: https://snapshot.ubuntu.com/ubuntu/${snapshot}/" \
       "Suites: noble noble-updates noble-security" \
       "Components: main universe" \
       "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg" \
       > /etc/apt/sources.list.d/ubuntu.sources \
    && DEBIAN_FRONTEND=noninteractive apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends ca-certificates sudo tini \
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
    && chmod 0555 /opt/gascan/artifacts/mise-linux-arm64 \
    && chmod 0444 /opt/gascan/artifacts/playwright-chromium-linux-arm64.zip

ENV HOME=/home/workspace
USER workspace:workspace
WORKDIR /workspace
VOLUME ["/opt/gascan/mise", "/home/workspace/.cache", "/home/workspace/.config/gascan"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/gascan-entrypoint"]
