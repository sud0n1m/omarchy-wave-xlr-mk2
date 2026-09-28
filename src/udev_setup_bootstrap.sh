# Passed literally as an argument to the system bash before sudo authentication.
# Never read a script or execute a binary from the user-writable checkout as root.
set -eu
[[ $# == 3 && $1 =~ ^/proc/[0-9]+/exe$ && $2 =~ ^[0-9a-f]{64}$ ]] || exit 1
[[ $3 == --install-udev-rule || $3 == --remove-udev-rule ]] || exit 1
umask 077
private_dir=$(/usr/bin/mktemp -d /tmp/omarchy-wave-xlr-setup.XXXXXXXXXX)
trap '/usr/bin/rm -r -- "$private_dir"' EXIT
/usr/bin/install -m 0700 -- "$1" "$private_dir/wave-xlr-control"
actual=$(/usr/bin/sha256sum -- "$private_dir/wave-xlr-control")
if [[ ${actual%% *} != "$2" ]]; then
    printf '%s\n' 'Setup executable changed; refusing privileged execution.' >&2
    exit 1
fi
# The directory and copy are root-owned and inaccessible to the invoking user.
# Execute only this verified copy; never re-open the original path for execution.
"$private_dir/wave-xlr-control" "$3"
