#!/bin/busybox sh
set -eu
export PATH=/bin

fail() {
    echo "vug stage 1: $* (root=${root:-unset})" >&2
    while :; do sleep 3600; done
}

trap 'fail "startup command failed"' EXIT
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
exec </dev/console >/dev/console 2>&1
mount -t tmpfs -o mode=755,nosuid,nodev tmpfs /run
set -f
root=
init=
for arg in $(cat /proc/cmdline); do
    case "$arg" in
        root=*) [ -z "$root" ] || fail "multiple root arguments"; root=${arg#root=} ;;
        init=*) [ -z "$init" ] || fail "multiple init arguments"; init=${arg#init=} ;;
    esac
done
set +f
case "$root" in
    PARTUUID=?*) uuid=$(printf '%s' "${root#PARTUUID=}" | tr A-F a-f) ;;
    *) fail "expected a PARTUUID" ;;
esac
case "$init" in
    /vug/store/*/init) ;;
    *) fail "expected a generation init" ;;
esac

systemd-udevd --daemon
while IFS= read -r module; do
    modprobe -- "$module"
done < /etc/force-modules
udevadm trigger --action=add --type=subsystems
udevadm trigger --action=add --type=devices
deadline=$(($(cut -d. -f1 /proc/uptime) + 30))
while :; do
    device=
    detected=
    for entry in /sys/class/block/*; do
        [ -f "$entry/partition" ] || continue
        node=/dev/${entry##*/}
        [ -b "$node" ] || continue
        found=$(blkid -p -s PART_ENTRY_UUID -o value "$node") || continue
        detected="$detected $node=$found"
        if [ "$found" = "$uuid" ]; then
            [ -z "$device" ] || fail "duplicate PARTUUID on $device and $node"
            device=$node
        fi
    done
    [ -z "$device" ] || break
    [ "$(cut -d. -f1 /proc/uptime)" -lt "$deadline" ] ||
        fail "partition did not appear within 30 seconds; detected:${detected:- none}"
    sleep 1
done
mount -t ext4 -o ro "$device" /new-root || fail "mounting $device"
[ -x "/new-root$init" ] || fail "generation init missing: $init"
udevadm control --exit
for dir in proc sys dev run; do
    mount -o move "/$dir" "/new-root/$dir"
done
echo "vug stage 1: switching root from $device ($root) to $init"
exec switch_root /new-root "$init"
