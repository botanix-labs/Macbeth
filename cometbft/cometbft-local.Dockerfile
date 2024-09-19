FROM --platform=linux/amd64 cometbft/cometbft:v1.x

WORKDIR /cometbft

COPY init.sh /tmp/init.sh

USER root
RUN chmod +x /tmp/init.sh
USER tmuser

ARG MONIKER
ARG PERSISTENT_PEERS
ARG P2P_LADDR
ARG P2P_ALLOW_DUPLICATE_IP
ARG PROXY_APP
ARG P2P_ADDR_BOOK_STRICT
ARG RPC_LADDR

ENTRYPOINT ["/bin/sh", "-c", "/tmp/init.sh"]
