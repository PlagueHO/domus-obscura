$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public static class DomusLabVolumeInspection
{
    [StructLayout(LayoutKind.Sequential)]
    struct StorageType { public uint DeviceId; public Guid VendorId; }
    [StructLayout(LayoutKind.Sequential)]
    struct OpenParameters {
        public uint Version;
        public int GetInfoOnly, ReadOnly;
        public Guid ResiliencyGuid;
    }
    [StructLayout(LayoutKind.Explicit, Size = 32)]
    struct DiskInfo {
        [FieldOffset(0)] public uint Version;
        [FieldOffset(8)] public ulong VirtualSize;
        [FieldOffset(8)] public uint SubtypeOrLoaded;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct Dependency {
        public uint Flags, ProviderFlags;
        public StorageType Storage;
        public uint AncestorLevel;
        public IntPtr Device, HostVolume, DependentVolume, RelativePath;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct FileId { public ulong Volume, Low, High; }
    [StructLayout(LayoutKind.Sequential)]
    struct FileAttributesInfo { public uint Attributes, ReparseTag; }
    [StructLayout(LayoutKind.Sequential)]
    struct FileSizeInfo {
        public long AllocationSize, EndOfFile;
        public uint Links;
        public byte DeletePending, Directory;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern SafeFileHandle CreateFileW(string path, uint access, uint share,
        IntPtr security, uint creation, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle file, int kind,
        out FileId info, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle file, int kind,
        out FileAttributesInfo info, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle file, int kind,
        out FileSizeInfo info, uint size);
    [DllImport("virtdisk.dll", CharSet = CharSet.Unicode)]
    static extern uint OpenVirtualDisk(ref StorageType storage, string path,
        uint access, uint flags, ref OpenParameters parameters, out SafeFileHandle handle);
    [DllImport("virtdisk.dll")]
    static extern uint GetVirtualDiskInformation(SafeFileHandle disk, ref uint size,
        ref DiskInfo info, IntPtr used);
    [DllImport("virtdisk.dll")]
    static extern uint GetStorageDependencyInformation(SafeFileHandle root, uint flags,
        uint size, IntPtr info, out uint used);

    static void Check(uint error, string operation) {
        if (error != 0) throw new Win32Exception((int)error, operation + " failed: " + error);
    }
    static SafeFileHandle OpenMetadata(string path, bool directory) {
        // Existing objects only. No write, attach, detach, or volume-management access.
        var file = CreateFileW(path, directory ? 0x80000000u : 0x80u, 7,
            IntPtr.Zero, 3, 0x00200000u | (directory ? 0x02000000u : 0u), IntPtr.Zero);
        if (file.IsInvalid) {
            int error = Marshal.GetLastWin32Error();
            file.Dispose();
            throw new Win32Exception(error, "Read-only metadata open failed: " + error);
        }
        return file;
    }
    static FileId Identity(SafeFileHandle file) {
        FileId result;
        if (!GetFileInformationByHandleEx(file, 18, out result, 24))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "128-bit file identity query failed");
        return result;
    }
    static void CheckImageAllocation(SafeFileHandle file, ulong virtualSize) {
        FileAttributesInfo attributes;
        FileSizeInfo size;
        if (!GetFileInformationByHandleEx(file, 9, out attributes, 8) ||
            !GetFileInformationByHandleEx(file, 1, out size, 24))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Backing image metadata query failed");
        if ((attributes.Attributes & (0x10u | 0x200u | 0x400u)) != 0 ||
            size.Directory != 0 || size.EndOfFile < 0 ||
            (ulong)size.EndOfFile < virtualSize || size.AllocationSize < size.EndOfFile)
            throw new InvalidOperationException("Use a fully allocated, nonsparse, regular image.");
    }
    static ulong FixedLoadedSize(string image) {
        var storage = new StorageType {
            DeviceId = 3, VendorId = new Guid("ec984aec-a0f9-47e9-901f-71415a66345b")
        };
        var parameters = new OpenParameters { Version = 2, GetInfoOnly = 1, ReadOnly = 1 };
        SafeFileHandle disk;
        Check(OpenVirtualDisk(ref storage, image, 0, 0, ref parameters, out disk), "OpenVirtualDisk");
        using (disk) {
            var info = new DiskInfo { Version = 7 };
            uint size = 32;
            Check(GetVirtualDiskInformation(disk, ref size, ref info, IntPtr.Zero), "VHDX subtype query");
            if (info.SubtypeOrLoaded != 2) throw new InvalidOperationException("A fixed VHDX is required.");
            info = new DiskInfo { Version = 13 };
            size = 32;
            Check(GetVirtualDiskInformation(disk, ref size, ref info, IntPtr.Zero), "VHDX attachment query");
            if (info.SubtypeOrLoaded != 1) throw new InvalidOperationException("The exact image is not attached.");
            info = new DiskInfo { Version = 1 };
            size = 32;
            Check(GetVirtualDiskInformation(disk, ref size, ref info, IntPtr.Zero), "VHDX size query");
            if (info.VirtualSize < 32UL * 1024 * 1024 || info.VirtualSize > 512UL * 1024 * 1024)
                throw new InvalidOperationException("Unsafe virtual disk size.");
            return info.VirtualSize;
        }
    }
    static string BoundedString(IntPtr address, IntPtr buffer, int size) {
        long offset = address.ToInt64() - buffer.ToInt64();
        if (address == IntPtr.Zero || offset < 0 || offset > size - 2 || (offset & 1) != 0)
            throw new InvalidOperationException("Invalid dependency string pointer.");
        int limit = (size - (int)offset) / 2;
        for (int length = 0; length < limit; length++)
            if (Marshal.ReadInt16(address, length * 2) == 0)
                return Marshal.PtrToStringUni(address, length);
        throw new InvalidOperationException("Unterminated dependency string.");
    }
    static void CheckDependency(SafeFileHandle root, FileId expected) {
        int size = 4096;
        for (int attempt = 0; attempt < 2; attempt++) {
            IntPtr buffer = Marshal.AllocHGlobal(size);
            try {
                Marshal.WriteInt32(buffer, 0, 2);
                Marshal.WriteInt32(buffer, 4, 0);
                uint used;
                uint error = GetStorageDependencyInformation(root, 1, (uint)size, buffer, out used);
                if (error == 122 && attempt == 0 && used > size && used <= 65536) {
                    size = (int)used;
                    continue;
                }
                Check(error, "Storage dependency query");
                if (Marshal.ReadInt32(buffer, 0) != 2 || Marshal.ReadInt32(buffer, 4) != 1 ||
                    8 + Marshal.SizeOf(typeof(Dependency)) > size)
                    throw new InvalidOperationException("Exactly one direct VHDX dependency is required.");
                var entry = (Dependency)Marshal.PtrToStructure(IntPtr.Add(buffer, 8), typeof(Dependency));
                if (entry.Storage.DeviceId != 3 || (entry.Flags & 2) == 0 ||
                    (entry.Flags & 0x31d) != 0)
                    throw new InvalidOperationException(String.Format(
                        "Unsafe VHDX dependency: flags=0x{0:x}, device={1}.", entry.Flags, entry.Storage.DeviceId));
                if ((entry.Flags & 0x40) != 0) {
                    // Azure can mark its running OS disk as removable. Accept only that exact host volume.
                    using (var system = OpenMetadata(Path.GetPathRoot(Environment.SystemDirectory), true))
                        if (Identity(system).Volume != expected.Volume)
                            throw new InvalidOperationException("A removable image host must be the running OS volume.");
                }
                string host = BoundedString(entry.HostVolume, buffer, size);
                string relative = BoundedString(entry.RelativePath, buffer, size);
                host = host.TrimEnd('\\');
                Guid hostGuid;
                if (!host.StartsWith(@"\\?\Volume{", StringComparison.OrdinalIgnoreCase) ||
                    !Guid.TryParseExact(host.Substring(10), "B", out hostGuid) ||
                    relative.Contains("..") || relative.Contains(":") || relative.Length == 0)
                    throw new InvalidOperationException(String.Format(
                        "Unsafe dependency backing path: host={0}, relative={1}.", host, relative));
                using (var backing = OpenMetadata(host + "\\" + relative.TrimStart('\\'), false)) {
                    FileId actual = Identity(backing);
                    if (actual.Volume != expected.Volume || actual.Low != expected.Low || actual.High != expected.High)
                        throw new InvalidOperationException("The fixture is not on the supplied image.");
                }
                return;
            } finally { Marshal.FreeHGlobal(buffer); }
        }
        throw new InvalidOperationException("Storage dependency query exceeded its bound.");
    }
    public static void Validate(string root, string image, ulong capacity) {
        if (new DriveInfo(Path.GetPathRoot(image)).DriveType != DriveType.Fixed)
            throw new InvalidOperationException("The image must be on a fixed local host volume.");
        ulong virtualSize = FixedLoadedSize(image);
        if (capacity > virtualSize) throw new InvalidOperationException("Volume exceeds the image size.");
        using (var file = OpenMetadata(image, false)) {
            CheckImageAllocation(file, virtualSize);
            using (var directory = OpenMetadata(root, true))
                CheckDependency(directory, Identity(file));
        }
    }
}
'@
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run application tests without elevation.'
}
$root = [IO.Path]::GetPathRoot($env:DOMUS_INSPECT_ROOT)
if ($root -notmatch '^[A-Za-z]:\\$') { throw 'A local drive-letter lab volume is required.' }
$volume = [IO.DriveInfo]::new($root)
$hostVolume = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($env:DOMUS_INSPECT_IMAGE))
[DomusLabVolumeInspection]::Validate($root, $env:DOMUS_INSPECT_IMAGE, [uint64]$volume.TotalSize)
@{
    capacity = [uint64]$volume.TotalSize
    filesystem = [string]$volume.DriveFormat
    host_available = [uint64]$hostVolume.AvailableFreeSpace
} | ConvertTo-Json -Compress
