FROM ubuntu:22.04

# The service does not need root: it listens on 8000 and writes only the compressed copies
# of the content - into the home folder of its own user. The user has a fixed numeric id,
# so Kubernetes can check 'runAsNonRoot' against it
RUN useradd --uid 10001 --user-group --create-home --home-dir /home/web-app-host \
    --shell /usr/sbin/nologin web-app-host

WORKDIR /app
COPY ./target/release/web-app-host ./web-app-host
COPY ./wwwroot ./wwwroot

USER 10001
ENV HOME=/home/web-app-host

ENTRYPOINT ["./web-app-host"]
