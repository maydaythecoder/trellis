#!/usr/bin/env bash
# #219: run the trellis-wsl self-hosted runner as the dedicated non-sudo user
# trellis-ci, under a sandboxed systemd unit, instead of as the owner.
#
# Run as root inside the distro, BEFORE scripts/runners/win-runner-account.ps1
# (which turns the trellis-runner-wsl scheduled task into the distro's
# keep-alive):
#
#   wsl.exe -d Ubuntu -u root -- bash scripts/runners/wsl-runner-account.sh --dry-run
#   wsl.exe -d Ubuntu -u root -- bash scripts/runners/wsl-runner-account.sh
#
# What it does (each step prints; --dry-run prints and changes nothing; a
# re-run only does what is still missing):
#   1. creates trellis-ci (own group, no sudo/adm/docker, locked password,
#      nologin shell);
#   2. moves /home/mayda/actions-runner to /home/trellis-ci/actions-runner
#      (the registration moves with it: no new runner token is needed);
#   3. rewrites the old path in .env, start.sh and ci-gitconfig, and drops
#      every /home/mayda and /mnt entry from .path (the job PATH), putting the
#      runner's own cargo/bin first (each file is backed up as *.pre219);
#   4. gives the directory to trellis-ci, mode 700, .credentials* 600;
#   5. installs and starts trellis-runner-wsl.service: User=trellis-ci,
#      NoNewPrivileges, the owner's home hidden, and /mnt/c, /mnt/wslg, /run/WSL, /init
#      inaccessible, so a job can neither read the Windows drives nor start a
#      Windows program (which would run with the owner's Windows token).
#
# --rollback undoes 2-5 and locks the account (it is not deleted).
# It never prints .credentials or any token.
set -euo pipefail

CI_USER=trellis-ci
OWNER=mayda
DRY=0
STOP=0
ROLLBACK=0
UNIT_NAME=trellis-runner-wsl.service
UNIT=/etc/systemd/system/$UNIT_NAME

usage() {
	cat <<'EOF'
usage: wsl-runner-account.sh [--dry-run] [--stop-runner] [--rollback]
                             [--user trellis-ci] [--owner mayda]
Run as root: wsl.exe -d Ubuntu -u root -- bash scripts/runners/wsl-runner-account.sh
EOF
}

while (($#)); do
	case $1 in
	--dry-run) DRY=1 ;;
	--stop-runner) STOP=1 ;;
	--rollback) ROLLBACK=1 ;;
	--user) CI_USER=$2; shift ;;
	--owner) OWNER=$2; shift ;;
	-h | --help) usage; exit 0 ;;
	*) usage >&2; exit 2 ;;
	esac
	shift
done

if [[ $EUID -ne 0 ]]; then
	echo "refusing: run as root (wsl.exe -d Ubuntu -u root -- bash $0 ...). Nothing was changed." >&2
	exit 1
fi

OLD_DIR=/home/$OWNER/actions-runner
NEW_HOME=/home/$CI_USER
NEW_DIR=$NEW_HOME/actions-runner

n=0
step() { n=$((n + 1)); printf '[%d] %s\n' "$n" "$*"; }
note() { printf '    %s\n' "$*"; }
run() {
	if ((DRY)); then
		printf '    would run: %s\n' "$*"
	else
		printf '    + %s\n' "$*"
		"$@"
	fi
}

listener_pids() { pgrep -f "^$1/bin/Runner.Listener( |$)" || true; }

assert_stopped() { # $1 = runner dir
	local pids
	pids=$(listener_pids "$1")
	if [[ -z $pids ]]; then note "no runner running from $1"; return; fi
	note "runner running from $1 (pid $(echo $pids))"
	if ((!STOP)); then
		if ((DRY)); then note 'a real run would refuse here: stop it first or pass --stop-runner'; return; fi
		echo "refusing: the runner in $1 is running. On Windows: schtasks /end /tn trellis-runner-wsl, wait for it to exit (or pass --stop-runner, which also cancels a job in progress)." >&2
		exit 1
	fi
	run kill -TERM $pids
	if ((!DRY)); then
		for _ in $(seq 30); do [[ -z $(listener_pids "$1") ]] && break; sleep 1; done
		[[ -z $(listener_pids "$1") ]] || { echo "runner did not stop" >&2; exit 1; }
	fi
}

backup_once() { # $1 = file
	[[ -e $1.pre219 ]] || run cp -p -- "$1" "$1.pre219"
}

# Rewrite every occurrence of $2 with $3 in file $1 (backup first).
rewrite_path() {
	local f=$1 from=$2 to=$3 hits
	[[ -f $f ]] || { note "$f: absent"; return; }
	hits=$(grep -c -F -- "$from" "$f" || true)
	if [[ $hits == 0 ]]; then note "$(basename "$f"): no $from"; return; fi
	note "$(basename "$f"): $hits line(s) name $from"
	backup_once "$f"
	run sed -i "s#$from#$to#g" "$f"
}

unit_text() {
	cat <<EOF
# Installed by scripts/runners/wsl-runner-account.sh (#219).
[Unit]
Description=trellis-wsl GitHub Actions runner as $CI_USER (#219)
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
User=$CI_USER
Group=$CI_USER
WorkingDirectory=$NEW_DIR
ExecStart=$NEW_DIR/start.sh
Restart=always
RestartSec=10
KillSignal=SIGTERM
TimeoutStopSec=5min
UMask=0077
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=full
ProtectHome=tmpfs
BindPaths=$NEW_HOME
# /mnt/c is the Windows drive with the owner's Windows token behind it, and
# /init + /run/WSL are interop: an .exe started from here runs as the owner on
# Windows. /mnt/wslg holds the owner's runtime dir. None of them is reachable
# from a job. /mnt/wsl stays: /etc/resolv.conf links into it. (Another drive
# mounted later under /mnt would need adding here.)
InaccessiblePaths=/mnt/c -/mnt/wslg -/run/WSL -/init

[Install]
WantedBy=multi-user.target
EOF
}

# =============================================================================
if ((ROLLBACK)); then
	echo "#219 rollback: trellis-wsl back to $OLD_DIR as $OWNER; $CI_USER locked"
	((DRY)) && echo '(--dry-run: nothing below is changed)'

	step "stop and remove $UNIT_NAME"
	if [[ -e $UNIT ]]; then
		run systemctl disable --now "$UNIT_NAME"
		run rm -f -- "$UNIT"
		run systemctl daemon-reload
	else
		note 'not installed'
	fi
	assert_stopped "$NEW_DIR"

	step "move the runner back to $OLD_DIR"
	if [[ -d $NEW_DIR && ! -e $OLD_DIR ]]; then
		run mv -- "$NEW_DIR" "$OLD_DIR"
	elif [[ -d $OLD_DIR ]]; then
		note 'already there'
	else
		echo "neither $NEW_DIR nor $OLD_DIR exists" >&2; exit 1
	fi

	step 'restore .env, start.sh, ci-gitconfig, .path from *.pre219'
	for f in .env start.sh ci-gitconfig .path; do
		if [[ -e $OLD_DIR/$f.pre219 || $DRY == 1 ]]; then
			run mv -f -- "$OLD_DIR/$f.pre219" "$OLD_DIR/$f"
		fi
	done

	step "give it back to $OWNER"
	run chown -R "$OWNER:$OWNER" -- "$OLD_DIR"
	run chmod 755 -- "$OLD_DIR"
	note '.credentials* stay 600 (they were world-readable before #219)'

	step "lock $CI_USER"
	if id -u "$CI_USER" >/dev/null 2>&1; then
		run usermod --lock --expiredate 1 "$CI_USER"
		note "to delete it as well: userdel -r $CI_USER"
	else
		note 'no such user'
	fi
	echo "done. Then run win-runner-account.ps1 -Rollback, which restores the old trellis-runner-wsl task."
	exit 0
fi

# =============================================================================
echo "#219: trellis-wsl runs as $CI_USER from $NEW_DIR under $UNIT_NAME"
((DRY)) && echo '(--dry-run: nothing below is changed)'

step "user $CI_USER: own group, no sudo, no password, no login shell"
if id -u "$CI_USER" >/dev/null 2>&1; then
	note 'exists'
	run usermod --expiredate '' --shell /usr/sbin/nologin "$CI_USER"
else
	run useradd --create-home --user-group --shell /usr/sbin/nologin \
		--comment "trellis CI runner (#219)" "$CI_USER"
fi
if id -u "$CI_USER" >/dev/null 2>&1; then
	run passwd --lock "$CI_USER"
	for g in sudo admin wheel adm docker lxd ollama "$OWNER"; do
		if id -nG "$CI_USER" | tr ' ' '\n' | grep -qx -- "$g"; then
			run gpasswd -d "$CI_USER" "$g"
		fi
	done
	note "groups: $(id -nG "$CI_USER")"
fi
if [[ -d $NEW_HOME ]]; then run chmod 750 -- "$NEW_HOME"; fi

step 'the runner must be stopped'
assert_stopped "$OLD_DIR"

step "move $OLD_DIR to $NEW_DIR"
if [[ -d $OLD_DIR && ! -e $NEW_DIR ]]; then
	[[ -f $OLD_DIR/.runner ]] || { echo "$OLD_DIR is not a configured runner (no .runner)" >&2; exit 1; }
	run mv -- "$OLD_DIR" "$NEW_DIR"
elif [[ -d $NEW_DIR && ! -e $OLD_DIR ]]; then
	note 'already moved'
elif [[ -e $OLD_DIR && -e $NEW_DIR ]]; then
	echo "refusing: both $OLD_DIR and $NEW_DIR exist; remove the one that is not the runner" >&2
	exit 1
else
	echo "no runner at $OLD_DIR or $NEW_DIR" >&2
	exit 1
fi
# Under --dry-run nothing moved: inspect the files where they are.
DIR=$NEW_DIR
[[ -d $NEW_DIR ]] || DIR=$OLD_DIR

step "rewrite $OLD_DIR -> $NEW_DIR in the runner's files"
for f in .env start.sh ci-gitconfig; do
	rewrite_path "$DIR/$f" "$OLD_DIR" "$NEW_DIR"
done

step ".path (the job PATH): drop /home/$OWNER and /mnt entries, runner cargo/bin first"
if [[ -f $DIR/.path ]]; then
	IFS=: read -r -a parts <"$DIR/.path" || true
	keep=("$NEW_DIR/cargo/bin")
	dropped=0
	for p in "${parts[@]}"; do
		case $p in
		"$NEW_DIR/cargo/bin" | '') ;;
		/home/* | /mnt/* | /root/*) dropped=$((dropped + 1)) ;;
		*) [[ " ${keep[*]} " == *" $p "* ]] || keep+=("$p") ;;
		esac
	done
	want=$(IFS=:; echo "${keep[*]}")
	if [[ $(<"$DIR/.path") == "$want" ]]; then
		note 'already done'
	else
		note "dropping $dropped entr(y/ies); new PATH: $want"
		backup_once "$DIR/.path"
		if ((DRY)); then
			note "would write $DIR/.path"
		else
			printf '%s' "$want" >"$DIR/.path"
		fi
	fi
else
	note 'no .path'
fi

step "owner $CI_USER, mode 700, .credentials* 600"
run chown -R "$CI_USER:$CI_USER" -- "$DIR"
run chmod 700 -- "$DIR"
for f in .credentials .credentials_rsaparams; do
	if [[ -e $DIR/$f ]]; then run chmod 600 -- "$DIR/$f"; fi
done

step "systemd unit $UNIT"
if [[ "$(cat /proc/1/comm 2>/dev/null)" != systemd ]]; then
	echo "systemd is not PID 1: set [boot] systemd=true in /etc/wsl.conf and run wsl --shutdown first" >&2
	exit 1
fi
changed=0
if [[ -f $UNIT ]] && [[ "$(<"$UNIT")" == "$(unit_text)" ]]; then
	note 'up to date'
else
	changed=1
	if ((DRY)); then
		note "would write $UNIT:"
		unit_text | sed 's/^/      | /'
	else
		note "writing $UNIT"
		unit_text >"$UNIT"
		chmod 644 "$UNIT"
	fi
	run systemctl daemon-reload
fi
run systemctl enable "$UNIT_NAME"
if ((changed)); then
	run systemctl restart "$UNIT_NAME"
else
	run systemctl start "$UNIT_NAME"
fi
if ((!DRY)); then
	sleep 3
	note "$(systemctl is-active "$UNIT_NAME"): $(systemctl show -p MainPID --value "$UNIT_NAME" | xargs -r ps -o user= -p 2>/dev/null || true)"
fi

echo
echo 'done. Next, from an elevated PowerShell: win-runner-account.ps1 (makes trellis-runner-wsl the keep-alive).'
echo "Check: systemctl status $UNIT_NAME; tail $NEW_DIR/runner.log"
