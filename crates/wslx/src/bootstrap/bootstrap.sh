# Run by wslx.exe as `sh -c <this> wslx <x86_64 hash> <aarch64 hash>` inside
# the distribution, with its stdin and stdout connected to wslx.exe. Lines for
# wslx.exe start with "@wslx "; see bootstrap.rs.
set -eu

case $(uname -m) in
x86_64 | amd64) arch=x86_64 hash=$1 ;;
aarch64 | arm64) arch=aarch64 hash=$2 ;;
*)
	echo "@wslx error no relay is built for $(uname -m)"
	exit 1
	;;
esac

dir=${XDG_CACHE_HOME:-$HOME/.cache}/zetta/wslx
relay=$dir/wslx-relay-$hash-$arch

if [ ! -x "$relay" ]; then
	mkdir -p "$dir"
	chmod 700 "$dir"
	partial=$relay.partial.$$
	echo "@wslx send $arch"
	# `read` takes the size line a byte at a time, and wslx.exe sends nothing
	# after the image until it sees "ready", so `head` cannot read past it.
	IFS= read -r size
	head -c "$size" >"$partial"
	received=$(wc -c <"$partial")
	if [ "$((received))" -ne "$size" ]; then
		rm -f "$partial"
		echo "@wslx error the relay transfer ended after $((received)) of $size bytes"
		exit 1
	fi
	chmod 700 "$partial"
	mv -f "$partial" "$relay"
	# Other builds' relays; a transfer still in progress has a suffix after
	# the architecture and is left alone.
	for stale in "$dir"/wslx-relay-*-"$arch"; do
		[ "$stale" = "$relay" ] || rm -f "$stale"
	done
fi

exec "$relay" serve
