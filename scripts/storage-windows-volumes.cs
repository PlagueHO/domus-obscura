// Generated-volume diagnostic only. No production binding or separation authority.
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

public static class StorageWindowsVolumes
{
    [StructLayout(LayoutKind.Sequential)]
    private struct StorageType { public uint Device; public Guid Vendor; }
    [StructLayout(LayoutKind.Explicit, Size = 128)]
    private struct CreateParameters
    {
        [FieldOffset(0)] public uint Version;
        [FieldOffset(8)] public Guid Id;
        [FieldOffset(24)] public ulong Size;
        [FieldOffset(32)] public uint Block;
        [FieldOffset(36)] public uint Sector;
        [FieldOffset(40)] public uint PhysicalSector;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct AttachParameters { public uint Version, Reserved; }
    [StructLayout(LayoutKind.Sequential)]
    private struct OpenParameters { public uint Version, Depth; }
    [StructLayout(LayoutKind.Explicit, Size = 32)]
    private struct DiskInformation
    {
        [FieldOffset(0)] public uint Version;
        [FieldOffset(8)] public ulong Size;
        [FieldOffset(8)] public uint Loaded;
        [FieldOffset(8)] public Guid Id;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct Dependency
    {
        public uint Flags, ProviderFlags;
        public StorageType Storage;
        public uint Level;
        public IntPtr Device, Host, Volume, Relative;
    }
    [DllImport("virtdisk.dll", CharSet = CharSet.Unicode)]
    private static extern uint CreateVirtualDisk(ref StorageType type, string path,
        uint access, IntPtr security, uint flags, uint provider,
        ref CreateParameters parameters, IntPtr overlapped, out SafeFileHandle disk);
    [DllImport("virtdisk.dll", CharSet = CharSet.Unicode)]
    private static extern uint OpenVirtualDisk(ref StorageType type, string path,
        uint access, uint flags, ref OpenParameters parameters, out SafeFileHandle disk);
    [DllImport("virtdisk.dll")]
    private static extern uint AttachVirtualDisk(SafeFileHandle disk, IntPtr security,
        uint flags, uint provider, ref AttachParameters parameters, IntPtr overlapped);
    [DllImport("virtdisk.dll")]
    private static extern uint DetachVirtualDisk(SafeFileHandle disk, uint flags, uint provider);
    [DllImport("virtdisk.dll", CharSet = CharSet.Unicode)]
    private static extern uint GetVirtualDiskPhysicalPath(SafeFileHandle disk,
        ref uint size, StringBuilder path);
    [DllImport("virtdisk.dll")]
    private static extern uint GetVirtualDiskInformation(SafeFileHandle disk,
        ref uint size, ref DiskInformation info, IntPtr used);
    [DllImport("virtdisk.dll")]
    private static extern uint GetStorageDependencyInformation(SafeFileHandle root,
        uint flags, uint size, IntPtr info, out uint used);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(string path, uint access,
        uint share, IntPtr security, uint creation, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandleEx(SafeFileHandle file,
        int kind, byte[] buffer, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool DeviceIoControl(SafeFileHandle file, uint code,
        IntPtr input, uint inputSize, byte[] output, uint outputSize,
        out uint used, IntPtr overlapped);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetFinalPathNameByHandleW(SafeFileHandle file,
        StringBuilder path, uint size, uint flags);

    private static void Check(uint error, string operation)
    {
        if (error != 0) throw new Win32Exception((int)error, operation + ": " + error);
    }
    private static SafeFileHandle Metadata(string path, bool volume)
    {
        SafeFileHandle file = CreateFileW(path, volume ? 0u : 0x80u,
            3, IntPtr.Zero, 3, volume ? 0u : 0x00200000u, IntPtr.Zero);
        if (file.IsInvalid)
        {
            int error = Marshal.GetLastWin32Error();
            file.Dispose();
            throw new Win32Exception(error);
        }
        return file;
    }
    private static string Identity(SafeFileHandle file)
    {
        byte[] bytes = new byte[24];
        if (!GetFileInformationByHandleEx(file, 18, bytes, 24))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        return BitConverter.ToString(bytes).Replace("-", "").ToLowerInvariant();
    }
    private static string ReadString(IntPtr address, IntPtr buffer, int size)
    {
        long offset = address.ToInt64() - buffer.ToInt64();
        if (address == IntPtr.Zero || offset < 0 || offset > size - 2 || (offset & 1) != 0)
            throw new IOException("Invalid dependency string.");
        int bound = (size - (int)offset) / 2;
        for (int length = 0; length < bound; length++)
            if (Marshal.ReadInt16(address, length * 2) == 0)
                return Marshal.PtrToStringUni(address, length);
        throw new IOException("Unterminated dependency string.");
    }

    public sealed class Image : IDisposable
    {
        private SafeFileHandle disk, file;
        public string Path { get; private set; }
        public Guid Id { get; private set; }
        public Guid InitialNamespaceIdentifier { get; private set; }
        public string FileIdentity { get; private set; }
        public bool Attached { get; private set; }
        public const ulong Capacity = 256UL * 1024 * 1024;

        internal Image(string path)
        {
            Path = path;
            InitialNamespaceIdentifier = Guid.NewGuid();
            var type = new StorageType {
                Device = 3, Vendor = new Guid("ec984aec-a0f9-47e9-901f-71415a66345b")
            };
            var parameters = new CreateParameters {
                Version = 2, Id = InitialNamespaceIdentifier, Size = Capacity,
                Block = 2 * 1024 * 1024, Sector = 512, PhysicalSector = 512
            };
            Check(CreateVirtualDisk(ref type, path, 0, IntPtr.Zero, 0, 0,
                ref parameters, IntPtr.Zero, out disk), "CreateVirtualDisk");
            try
            {
                file = Metadata(path, false);
                FileIdentity = Identity(file);
                if (ReadIdentifier(2) != InitialNamespaceIdentifier)
                    throw new IOException("Unexpected initial disk namespace identifier.");
                Id = ReadIdentifier(14);
                if (Id == Guid.Empty) throw new IOException("Missing virtual disk ownership identifier.");
                CheckInformation(false);
            }
            catch { disk.Dispose(); if (file != null) file.Dispose(); throw; }
        }

        private Guid ReadIdentifier(uint version)
        {
            var info = new DiskInformation { Version = version };
            uint size = 32;
            Check(GetVirtualDiskInformation(disk, ref size, ref info, IntPtr.Zero), "Disk identifier");
            return info.Id;
        }
        public Guid NamespaceIdentifier()
        {
            CheckInformation(Attached);
            return ReadIdentifier(2);
        }
        public string BackingCoordinate()
        {
            CheckInformation(Attached);
            var path = new StringBuilder(32769);
            uint size = GetFinalPathNameByHandleW(file, path, 32769, 1);
            if (size == 0) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (size >= 32769 || size != path.Length)
                throw new IOException("Incomplete backing-file coordinate.");
            return path.ToString();
        }
        private void CheckInformation(bool loaded)
        {
            var info = new DiskInformation { Version = 1 };
            uint size = 32;
            Check(GetVirtualDiskInformation(disk, ref size, ref info, IntPtr.Zero), "Disk size");
            if (info.Size != Capacity) throw new IOException("Unexpected disk capacity.");
            if (ReadIdentifier(14) != Id) throw new IOException("Unexpected virtual disk ownership identifier.");
            info = new DiskInformation { Version = 13 };
            size = 32;
            Check(GetVirtualDiskInformation(disk, ref size, ref info, IntPtr.Zero), "Disk attachment");
            if (info.Loaded != (loaded ? 1u : 0u)) throw new IOException("Unexpected attachment state.");
            using (SafeFileHandle current = Metadata(Path, false))
                if (Identity(current) != FileIdentity || Identity(file) != FileIdentity)
                    throw new IOException("Backing image was replaced.");
        }
        public void Attach()
        {
            CheckInformation(false);
            var parameters = new AttachParameters { Version = 1 };
            // No drive letters, no permanent lifetime, no parent/source image.
            Check(AttachVirtualDisk(disk, IntPtr.Zero, 2, 0, ref parameters, IntPtr.Zero),
                "AttachVirtualDisk");
            Attached = true;
            CheckInformation(true);
        }
        public uint DiskNumber()
        {
            CheckInformation(true);
            var path = new StringBuilder(260);
            uint size = 520;
            Check(GetVirtualDiskPhysicalPath(disk, ref size, path), "Physical disk path");
            const string prefix = @"\\.\PhysicalDrive";
            uint number;
            if (!path.ToString().StartsWith(prefix, StringComparison.Ordinal)
                || !UInt32.TryParse(path.ToString().Substring(prefix.Length), out number))
                throw new IOException("Invalid generated disk path.");
            return number;
        }
        public void VerifySavedBinding(string expectedFileIdentity, Guid expectedDiskId)
        {
            if (String.IsNullOrEmpty(expectedFileIdentity) || expectedDiskId == Guid.Empty)
                throw new IOException("Missing saved backing binding.");
            CheckInformation(Attached);
            if (FileIdentity != expectedFileIdentity || Id != expectedDiskId)
                throw new IOException("Saved backing binding does not match the actual image.");
        }
        public void Detach()
        {
            if (!Attached) return;
            CheckInformation(true);
            Check(DetachVirtualDisk(disk, 0, 0), "DetachVirtualDisk");
            Attached = false;
            CheckInformation(false);
        }
        public void Dispose()
        {
            try { Detach(); }
            finally { if (file != null) file.Dispose(); if (disk != null) disk.Dispose(); }
        }
    }
    public static Image Create(string path)
    {
        if (!System.IO.Path.IsPathRooted(path) || System.IO.Path.GetExtension(path) != ".vhdx"
            || File.Exists(path)) throw new IOException("A fresh absolute VHDX path is required.");
        return new Image(path);
    }
    public sealed class DetachedImageEvidence
    {
        public string FileIdentity { get; set; }
        public Guid VirtualDiskId { get; set; }
        public Guid NamespaceIdentifier { get; set; }
        public ulong Capacity { get; set; }
        public bool Loaded { get; set; }
    }
    public static DetachedImageEvidence InspectDetachedCopy(string path)
    {
        using (SafeFileHandle file = Metadata(path, false))
        {
            var result = new DetachedImageEvidence { FileIdentity = Identity(file) };
            var type = new StorageType {
                Device = 3, Vendor = new Guid("ec984aec-a0f9-47e9-901f-71415a66345b")
            };
            // Version one, depth zero and GET_INFO only. Never attach or write the copy.
            var parameters = new OpenParameters { Version = 1, Depth = 0 };
            SafeFileHandle disk;
            Check(OpenVirtualDisk(ref type, path, 0x00080000, 1, ref parameters, out disk),
                "Open copied image for information");
            using (disk)
            {
                foreach (uint version in new uint[] { 1, 2, 13, 14 })
                {
                    uint size = 32;
                    var info = new DiskInformation { Version = version };
                    Check(GetVirtualDiskInformation(disk, ref size, ref info, IntPtr.Zero),
                        "Copied image information");
                    if (version == 1) result.Capacity = info.Size;
                    else if (version == 2) result.NamespaceIdentifier = info.Id;
                    else if (version == 13) result.Loaded = info.Loaded != 0;
                    else result.VirtualDiskId = info.Id;
                }
                if (Identity(file) != result.FileIdentity ||
                    result.Capacity != Image.Capacity || result.Loaded)
                    throw new IOException("Copied image identity, capacity or attachment changed.");
            }
            return result;
        }
    }
    public static void VerifyDetachedCopyBinding(DetachedImageEvidence actual,
        string expectedFileIdentity, Guid expectedDiskId)
    {
        if (actual == null || actual.Loaded || actual.Capacity != Image.Capacity
            || String.IsNullOrEmpty(expectedFileIdentity) || expectedDiskId == Guid.Empty
            || actual.FileIdentity != expectedFileIdentity || actual.VirtualDiskId != expectedDiskId)
            throw new IOException("Saved backing binding does not match the detached copy.");
    }
    public sealed class VolumeEvidence
    {
        public string RootIdentity { get; set; }
        public string Coordinate { get; set; }
        public uint Disk { get; set; }
        public long Offset { get; set; }
        public long Length { get; set; }
        public string BackingIdentity { get; set; }
        public string BackingPath { get; set; }
        public uint DependencyFlags { get; set; }
        public uint DependencyLevel { get; set; }
    }
    public static VolumeEvidence Inspect(StorageWindowsPrototype.Root root, Image image)
    {
        root.Recheck();
        StorageWindowsPrototype.Evidence actual = root.ReadRetained();
        string volumePath = actual.Coordinate.Substring(0, 48);
        var result = new VolumeEvidence {
            RootIdentity = actual.Identity, Coordinate = actual.Coordinate
        };
        using (SafeFileHandle volume = Metadata(volumePath, true))
        {
            byte[] extents = new byte[4096];
            uint used;
            if (!DeviceIoControl(volume, 0x00560000, IntPtr.Zero, 0, extents,
                (uint)extents.Length, out used, IntPtr.Zero))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (used < 32 || BitConverter.ToUInt32(extents, 0) != 1)
                throw new IOException("Exactly one complete volume extent is required.");
            result.Disk = BitConverter.ToUInt32(extents, 8);
            result.Offset = BitConverter.ToInt64(extents, 16);
            result.Length = BitConverter.ToInt64(extents, 24);
            if (result.Disk != image.DiskNumber() || result.Offset < 0 || result.Length <= 0
                || result.Offset > (long)Image.Capacity - result.Length)
                throw new IOException("Volume extent is outside the exact generated disk.");
        }
        int size = 4096;
        for (int attempt = 0; attempt < 2; attempt++)
        {
            IntPtr buffer = Marshal.AllocHGlobal(size);
            try
            {
                Marshal.Copy(new byte[size], 0, buffer, size);
                Marshal.WriteInt32(buffer, 0, 2);
                uint used;
                uint error = GetStorageDependencyInformation(root.RetainedHandle, 1,
                    (uint)size, buffer, out used);
                if (error == 122 && attempt == 0 && used > size && used <= 65536)
                { size = (int)used; continue; }
                Check(error, "Storage dependency");
                if (Marshal.ReadInt32(buffer, 0) != 2 || Marshal.ReadInt32(buffer, 4) != 1
                    || size < 8 + Marshal.SizeOf(typeof(Dependency)))
                    throw new IOException("Exactly one complete dependency is required.");
                var entry = (Dependency)Marshal.PtrToStructure(IntPtr.Add(buffer, 8), typeof(Dependency));
                // The generated images are dynamic, not FULLY_ALLOCATED.
                // Removable requires the independently checked running OS host below.
                if (entry.Storage.Device != 3 || (entry.Flags & ~0xe0u) != 0)
                    throw new IOException(String.Format(
                        "Unsupported image dependency: device={0}, flags=0x{1:x}, level={2}.",
                        entry.Storage.Device, entry.Flags, entry.Level));
                string host = ReadString(entry.Host, buffer, size).TrimEnd('\\');
                string relative = ReadString(entry.Relative, buffer, size).TrimStart('\\');
                Guid guid;
                if (!host.StartsWith(@"\\?\Volume{", StringComparison.OrdinalIgnoreCase)
                    || host.Length != 48 || !Guid.TryParseExact(host.Substring(10), "B", out guid)
                    || relative.Length == 0 || relative.Contains("..") || relative.Contains(":"))
                    throw new IOException("Invalid backing image coordinate.");
                if ((entry.Flags & 0x40) != 0)
                {
                    string systemPath = System.IO.Path.GetPathRoot(Environment.SystemDirectory);
                    using (StorageWindowsPrototype.Root system = StorageWindowsPrototype.Open(systemPath))
                        if (!String.Equals(host, system.Evidence.Coordinate.TrimEnd('\\'),
                            StringComparison.OrdinalIgnoreCase)
                            || new DriveInfo(systemPath).DriveType != DriveType.Fixed)
                            throw new IOException("Removable backing is not the exact fixed local OS volume.");
                }
                result.BackingPath = host + "\\" + relative;
                using (SafeFileHandle backing = Metadata(result.BackingPath, false))
                    result.BackingIdentity = Identity(backing);
                if (result.BackingIdentity != image.FileIdentity)
                    throw new IOException("Volume refers to a different image.");
                result.DependencyFlags = entry.Flags;
                result.DependencyLevel = entry.Level;
                root.Recheck();
                return result;
            }
            finally { Marshal.FreeHGlobal(buffer); }
        }
        throw new IOException("Storage dependency exceeded the response bound.");
    }
}
