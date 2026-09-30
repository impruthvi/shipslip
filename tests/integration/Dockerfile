FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-server bash git ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir /run/sshd \
    && useradd --create-home --shell /bin/bash deploy \
    && printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\n' \
        > /etc/ssh/sshd_config.d/test.conf

# origin.git has two commits; tests clone it and reset to the first.
USER deploy
RUN git config --global user.email test@example.com \
    && git config --global user.name test \
    && git config --global init.defaultBranch main \
    && git init -q --bare /home/deploy/origin.git \
    && git clone -q /home/deploy/origin.git /tmp/seed \
    && cd /tmp/seed \
    && echo one > file && git add file && git commit -qm "First" \
    && echo two > file && git commit -qam "Second" \
    && git push -q origin main \
    && rm -rf /tmp/seed

USER root
EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
