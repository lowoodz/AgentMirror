#!/usr/bin/env bash
# Windows UTM guest access via SSH (config: config/test.env → SMR_WINDOWS_* / SMR_GUEST_STAGING).
# Usage: source scripts/vm/vm-ssh.sh && vm_ssh_require && vm_ssh "hostname"

_vm_ssh_root() {
  cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd
}

vm_ssh_init() {
  if [[ "${VM_SSH_INIT_DONE:-}" == 1 ]]; then
    return 0
  fi
  local root
  root="$(_vm_ssh_root)"
  if [[ -n "${SMR_VM_BACKEND:-}" && "${SMR_VM_BACKEND}" != ssh ]]; then
    echo "ERROR: only SSH backend is supported (SMR_VM_BACKEND=ssh); got '${SMR_VM_BACKEND}'" >&2
    exit 1
  fi
  # shellcheck source=load_test_env.sh
  source "${root}/scripts/load_test_env.sh"
  : "${SMR_WINDOWS_HOST:=windows-vm}"
  if [[ -z "${SMR_WINDOWS_USER:-}" ]]; then
    echo "ERROR: set SMR_WINDOWS_USER in config/test.env (copy from config/test.env.example)" >&2
    exit 1
  fi
  if [[ -z "${SMR_GUEST_STAGING:-}" ]]; then
    SMR_GUEST_STAGING="C:/Users/${SMR_WINDOWS_USER}/smr-staging"
  fi
  VM_HOST="$SMR_WINDOWS_HOST"
  VM_USER="$SMR_WINDOWS_USER"
  # Use SSH Host alias only — User/IdentityFile come from ~/.ssh/config (user@host breaks Host matching).
  VM_SSH="$VM_HOST"
  VM_BACKEND=ssh
  VM_SSH_CTRL="${TMPDIR:-/tmp}/smr-vm-ctrl-${VM_HOST}"
  VM_SSH_MASTER_OPTS=(
    -o BatchMode=yes
    -o ConnectTimeout=20
    -o ControlMaster=auto
    -o "ControlPath=${VM_SSH_CTRL}"
    -o ControlPersist=600
  )
  VM_SSH_MUX_OPTS=(
    -o BatchMode=yes
    -o ConnectTimeout=20
    -o ControlMaster=no
    -o "ControlPath=${VM_SSH_CTRL}"
  )
  VM_SSH_OPTS=("${VM_SSH_MASTER_OPTS[@]}")
  VM_SSH_INIT_DONE=1
  GUEST_STAGING="$SMR_GUEST_STAGING"
  export VM_HOST VM_USER VM_SSH VM_BACKEND VM_SSH_OPTS GUEST_STAGING SMR_GUEST_STAGING
}

vm_ssh_close() {
  vm_ssh_init
  ssh -O exit -o "ControlPath=${VM_SSH_CTRL}" "${VM_SSH}" 2>/dev/null || true
  VM_SSH_CONNECTED=0
}

vm_ssh_require() {
  vm_ssh_init
  [[ "${VM_SSH_CONNECTED:-}" == 1 ]] && return 0
  if ! ssh "${VM_SSH_MASTER_OPTS[@]}" "$VM_SSH" "echo ok" >/dev/null 2>&1; then
    vm_ssh_close
    echo "ERROR: cannot SSH to Host ${VM_HOST} (user ${VM_USER}) — start UTM guest and verify ~/.ssh/config" >&2
    exit 1
  fi
  local remote_user
  remote_user="$(ssh "${VM_SSH_MUX_OPTS[@]}" "$VM_SSH" "cmd.exe /c echo %USERNAME%" 2>/dev/null | tr -d '\r\n' || true)"
  if [[ -n "$remote_user" && "$remote_user" != "$VM_USER" ]]; then
    VM_USER="$remote_user"
    if [[ "${SMR_GUEST_STAGING:-}" == *windows-user* || -z "${SMR_GUEST_STAGING:-}" ]]; then
      GUEST_STAGING="C:/Users/${VM_USER}/smr-staging"
      SMR_GUEST_STAGING="$GUEST_STAGING"
    fi
    export VM_USER GUEST_STAGING SMR_GUEST_STAGING
  fi
  VM_SSH_CONNECTED=1
  vm_ssh_mkdir "$GUEST_STAGING"
}

vm_ssh_bg() {
  vm_ssh_init
  vm_ssh_require
  ssh "${VM_SSH_MUX_OPTS[@]}" -o ServerAliveInterval=30 -o ServerAliveCountMax=120 "$VM_SSH" "$@" &
  VM_SSH_BG_PID=$!
  export VM_SSH_BG_PID
}

vm_ssh() {
  vm_ssh_init
  vm_ssh_require
  ssh "${VM_SSH_MUX_OPTS[@]}" "$VM_SSH" "$@"
}

vm_scp_to() {
  local local_path="$1" remote_path="$2"
  vm_ssh_init
  vm_ssh_require
  scp -q "${VM_SSH_MUX_OPTS[@]}" "$local_path" "${VM_SSH}:${remote_path}"
}

vm_scp_from() {
  local remote_path="$1" local_path="$2"
  vm_ssh_init
  vm_ssh_require
  scp -q "${VM_SSH_MUX_OPTS[@]}" "${VM_SSH}:${remote_path}" "$local_path"
}

vm_ssh_mkdir() {
  local dir="$1"
  vm_ssh_init
  vm_ssh "powershell -NoProfile -Command \"New-Item -ItemType Directory -Force -Path '${dir}' | Out-Null\"" 2>/dev/null || true
}
