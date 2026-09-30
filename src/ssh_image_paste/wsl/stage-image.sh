set -eu
umask 077
directory="$(mktemp -d /tmp/zetta-image.XXXXXXXX)"
trap 'rm -rf -- "$directory"' 0
image_path="$directory/image.png"
cat > "$image_path"
# An interrupted Windows-side pipe must not commit a truncated image.
[ "$(wc -c < "$image_path")" -eq "$1" ]
printf '%s%s%s\n' "$2" "$image_path" "$2"
trap - 0
