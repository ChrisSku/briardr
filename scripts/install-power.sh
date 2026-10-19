#!/bin/sh
# Lets Briardr keep the Mac awake with the lid closed (Settings → Mac power → Always on).
# Installs a sudoers rule that allows exactly `pmset disablesleep 1` and `pmset disablesleep 0` without a
# password, nothing else. Remove it with: sudo rm /etc/sudoers.d/briardr-power
set -eu
rule=$(mktemp)
trap 'rm -f "$rule"' EXIT
printf '%s ALL=(root) NOPASSWD: /usr/bin/pmset disablesleep 1, /usr/bin/pmset disablesleep 0\n' "$(id -un)" >"$rule"
/usr/sbin/visudo -cqf "$rule"
echo "Installing /etc/sudoers.d/briardr-power (asks for your password once)."
sudo /usr/bin/install -m 0440 -o root -g wheel "$rule" /etc/sudoers.d/briardr-power
echo "Done. Briardr can now keep the Mac awake with the lid closed."
