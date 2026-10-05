#!/bin/bash
# Built-in script: enable the OpenSSH server at boot (needs the "openssh" package).
if ! systemctl cat sshd.service >/dev/null 2>&1; then
    echo "sshd.service does not exist: add the \"openssh\" package"
    exit 1
fi
systemctl enable sshd.service
