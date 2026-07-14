ARG BASE_IMAGE
FROM ${BASE_IMAGE}

ARG UBUNTU_SNAPSHOT

COPY .artifacts/mise-linux-arm64 /opt/gascan/artifacts/mise-linux-arm64
COPY .artifacts/playwright-chromium-linux-arm64.zip /opt/gascan/artifacts/playwright-chromium-linux-arm64.zip

RUN snapshot="$(printf '%s' "${UBUNTU_SNAPSHOT}" | tr -d -- '-:')" \
    && printf '%s\n' \
       "Types: deb" \
       "URIs: https://snapshot.ubuntu.com/ubuntu/${snapshot}/" \
       "Suites: noble noble-updates noble-security" \
       "Components: main universe" \
       "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg" \
       > /etc/apt/sources.list.d/ubuntu.sources \
    && DEBIAN_FRONTEND=noninteractive apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && chmod 0555 /opt/gascan/artifacts/mise-linux-arm64 \
    && chmod 0444 /opt/gascan/artifacts/playwright-chromium-linux-arm64.zip
