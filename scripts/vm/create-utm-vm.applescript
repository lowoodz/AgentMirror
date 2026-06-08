-- Create SafeRoute Windows x86_64 test VM in UTM (Apple Silicon: QEMU emulation).
-- Env: SMR_VM_NAME (default SafeRoute-Win11-x64), SMR_VM_ISO (required path)

on run
	set vmName to system attribute "SMR_VM_NAME"
	if vmName is missing value or vmName is "" then set vmName to "SafeRoute-Win11-x64"

	set isoPath to system attribute "SMR_VM_ISO"
	if isoPath is missing value or isoPath is "" then
		error "Set SMR_VM_ISO to the Windows 11 x64 ISO path"
	end if
	set isoFile to POSIX file isoPath

	tell application "UTM"
		-- Skip if VM already exists
		try
			set existing to virtual machine named vmName
			return "VM already exists: " & vmName
		end try

		set newVM to make new virtual machine with properties {backend:qemu, configuration:{¬
			name:vmName, ¬
			notes:"SafeRoute Windows x86_64 test VM (OpenSSH / windows_vm_test.sh)", ¬
			architecture:"x86_64", ¬
			memory:8192, ¬
			cpu cores:4, ¬
			hypervisor:false, ¬
			drives:{{removable:true, source:isoFile}, {guest size:65536}}, ¬
			network interfaces:{{mode:bridged}}, ¬
			displays:{{hardware:"virtio-gpu-gl-pci"}} ¬
		}}
	end tell

	return "Created VM: " & vmName & " — start it in UTM and install Windows from ISO"
end run
