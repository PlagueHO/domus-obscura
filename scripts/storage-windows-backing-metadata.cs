// Read-only diagnostic. Native metadata is not production binding authority.
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public static class StorageWindowsBackingMetadata
{
    public const int MaximumDevices = 8;
    public const int MaximumResponse = 65536;

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(string path, uint access,
        uint share, IntPtr security, uint creation, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool DeviceIoControl(SafeFileHandle file, uint code,
        byte[] input, uint inputSize, byte[] output, uint outputSize,
        out uint used, IntPtr overlapped);
    [DllImport("virtdisk.dll")]
    private static extern uint GetStorageDependencyInformation(SafeFileHandle root,
        uint flags, uint size, IntPtr buffer, out uint used);

    public sealed class Extent
    {
        public uint Disk { get; set; }
        public long Offset { get; set; }
        public long Length { get; set; }
    }

    public sealed class QueryEvidence
    {
        public uint Control { get; set; }
        public uint Property { get; set; }
        public string Status { get; set; }
        public uint Win32Error { get; set; }
        public uint ReturnedBytes { get; set; }
        public string BytesBase64 { get; set; }
    }

    public sealed class DeviceEvidence
    {
        public uint Disk { get; set; }
        public uint OpenError { get; set; }
        public bool NumberCorrelated { get; set; }
        public QueryEvidence[] Properties { get; set; }
    }

    public sealed class DependencyEvidence
    {
        public uint Win32Error { get; set; }
        public uint ReturnedBytes { get; set; }
        public uint Version { get; set; }
        public uint Entries { get; set; }
        public string Status { get; set; }
    }

    public sealed class Evidence
    {
        public StorageWindowsPrototype.Evidence Root { get; set; }
        public Extent[] Extents { get; set; }
        public DeviceEvidence[] Devices { get; set; }
        public DependencyEvidence Dependencies { get; set; }
        public bool SnapshotRechecked { get; set; }
        public bool BindingAdmitted { get { return false; } }
    }

    public static Extent[] DecodeExtents(byte[] bytes, uint used)
    {
        if (bytes == null || used < 8 || used > bytes.Length || used > MaximumResponse)
            throw new IOException("Incomplete or oversized volume extents.");
        uint count = BitConverter.ToUInt32(bytes, 0);
        if (count == 0 || count > MaximumDevices || used != 8 + count * 24)
            throw new IOException("Volume extents exceed the approved count or response shape.");
        var result = new Extent[count];
        for (int i = 0; i < count; i++)
        {
            int position = 8 + i * 24;
            long offset = BitConverter.ToInt64(bytes, position + 8);
            long length = BitConverter.ToInt64(bytes, position + 16);
            if (offset < 0 || length <= 0 || offset > Int64.MaxValue - length)
                throw new IOException("Invalid volume extent range.");
            result[i] = new Extent {
                Disk = BitConverter.ToUInt32(bytes, position), Offset = offset, Length = length
            };
        }
        return result;
    }

    public static uint DecodeDescriptorSize(byte[] bytes, uint used)
    {
        if (bytes == null || used < 8 || used > bytes.Length || used > MaximumResponse)
            throw new IOException("Incomplete or oversized storage descriptor.");
        uint size = BitConverter.ToUInt32(bytes, 4);
        if (size < 8 || size > used || size > MaximumResponse)
            throw new IOException("Storage descriptor exceeds the returned response.");
        return size;
    }

    private static QueryEvidence Query(SafeFileHandle file, uint code, byte[] input,
        int capacity)
    {
        if (capacity < 8 || capacity > MaximumResponse)
            throw new IOException("Query exceeds the approved response bound.");
        byte[] output = new byte[capacity];
        uint used;
        bool success = DeviceIoControl(file, code, input,
            input == null ? 0u : (uint)input.Length, output, (uint)capacity,
            out used, IntPtr.Zero);
        uint error = success ? 0u : (uint)Marshal.GetLastWin32Error();
        if (success && (used == 0 || used > capacity))
            throw new IOException("Invalid native response length.");
        return new QueryEvidence {
            Control = code, Win32Error = error, ReturnedBytes = used,
            Status = success ? "observed" :
                error == 122 || error == 234 ? "bounded_refusal" : "unavailable",
            BytesBase64 = success ? Convert.ToBase64String(output, 0, (int)used) : null
        };
    }

    private static DeviceEvidence ReadDevice(uint number)
    {
        var result = new DeviceEvidence { Disk = number };
        using (SafeFileHandle file = CreateFileW(@"\\.\PhysicalDrive" + number, 0, 3,
            IntPtr.Zero, 3, 0, IntPtr.Zero))
        {
            if (file.IsInvalid)
            {
                result.OpenError = (uint)Marshal.GetLastWin32Error();
                return result;
            }
            QueryEvidence actual = Query(file, 0x002d1080, null, 12);
            if (actual.Status != "observed")
                throw new Win32Exception((int)actual.Win32Error, "Device number query");
            byte[] identity = Convert.FromBase64String(actual.BytesBase64);
            if (identity.Length != 12 || BitConverter.ToUInt32(identity, 0) != 7 ||
                BitConverter.ToUInt32(identity, 4) != number)
                throw new IOException("Opened device does not match the actual volume extent.");
            result.NumberCorrelated = true;
            var properties = new List<QueryEvidence>();
            foreach (uint property in new uint[] { 0, 2, 3 })
            {
                byte[] query = new byte[12];
                Array.Copy(BitConverter.GetBytes(property), query, 4);
                QueryEvidence descriptor = Query(file, 0x002d1400, query, MaximumResponse);
                descriptor.Property = property;
                if (descriptor.Status == "observed")
                    DecodeDescriptorSize(Convert.FromBase64String(descriptor.BytesBase64),
                        descriptor.ReturnedBytes);
                properties.Add(descriptor);
            }
            result.Properties = properties.ToArray();
            return result;
        }
    }

    private static DependencyEvidence ReadDependencies(SafeFileHandle root)
    {
        IntPtr buffer = Marshal.AllocHGlobal(MaximumResponse);
        try
        {
            Marshal.Copy(new byte[MaximumResponse], 0, buffer, MaximumResponse);
            Marshal.WriteInt32(buffer, 0, 2);
            uint used;
            uint error = GetStorageDependencyInformation(root, 1, MaximumResponse, buffer, out used);
            var result = new DependencyEvidence {
                Win32Error = error, ReturnedBytes = used,
                Status = error == 0 ? "header_observed_only" :
                    error == 122 || error == 234 ? "bounded_refusal" : "unavailable"
            };
            if (error == 0)
            {
                if (used < 8 || used > MaximumResponse)
                    throw new IOException("Incomplete storage dependency header.");
                result.Version = (uint)Marshal.ReadInt32(buffer, 0);
                result.Entries = (uint)Marshal.ReadInt32(buffer, 4);
                if (result.Version != 2 || result.Entries > MaximumDevices)
                    throw new IOException("Unsupported dependency version or excessive count.");
            }
            return result;
        }
        finally { Marshal.FreeHGlobal(buffer); }
    }

    public static Evidence Collect()
    {
        using (StorageWindowsPrototype.Root root = StorageWindowsPrototype.Open(@"C:\"))
        {
            root.Recheck();
            StorageWindowsPrototype.Evidence observed = root.ReadRetained();
            if (observed.Remote || observed.Filesystem != "NTFS" ||
                observed.Coordinate.Length != 49 ||
                !observed.Coordinate.StartsWith(@"\\?\Volume{", StringComparison.Ordinal))
                throw new IOException("Approved C: is not the supported local NTFS volume root.");
            string volumePath = observed.Coordinate.Substring(0, 48);
            using (StorageWindowsPrototype.Root named = StorageWindowsPrototype.Open(volumePath + "\\"))
            using (SafeFileHandle volume = CreateFileW(volumePath, 0, 3, IntPtr.Zero, 3, 0, IntPtr.Zero))
            {
                if (volume.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
                if (named.ReadRetained().Identity != observed.Identity)
                    throw new IOException("Volume GUID anchor does not match retained C: identity.");
                QueryEvidence first = Query(volume, 0x00560000, null, MaximumResponse);
                if (first.Status != "observed")
                    throw new Win32Exception((int)first.Win32Error, "Volume extents query");
                Extent[] extents = DecodeExtents(Convert.FromBase64String(first.BytesBase64),
                    first.ReturnedBytes);
                var devices = new List<DeviceEvidence>();
                var numbers = new HashSet<uint>();
                foreach (Extent extent in extents)
                    if (numbers.Add(extent.Disk)) devices.Add(ReadDevice(extent.Disk));
                var result = new Evidence {
                    Root = observed, Extents = extents, Devices = devices.ToArray(),
                    Dependencies = ReadDependencies(root.RetainedHandle)
                };
                QueryEvidence second = Query(volume, 0x00560000, null, MaximumResponse);
                if (second.Status != "observed" || first.BytesBase64 != second.BytesBase64)
                    throw new IOException("Live volume extents changed during metadata collection.");
                root.Recheck();
                named.Recheck();
                result.SnapshotRechecked = true;
                return result;
            }
        }
    }
}
