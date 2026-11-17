// Approved read-only diagnostic. PnP ancestry is not a storage backing graph.
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

public static class StorageWindowsTopology
{
    [DllImport("cfgmgr32.dll", CharSet = CharSet.Unicode)]
    private static extern uint CM_Locate_DevNodeW(out uint node, string id, uint flags);
    [DllImport("cfgmgr32.dll")]
    private static extern uint CM_Get_Parent(out uint parent, uint node, uint flags);
    [DllImport("cfgmgr32.dll", CharSet = CharSet.Unicode)]
    private static extern uint CM_Get_Device_IDW(uint node, StringBuilder id, uint length, uint flags);
    [DllImport("cfgmgr32.dll", CharSet = CharSet.Unicode)]
    private static extern uint CM_Get_DevNode_Registry_PropertyW(uint node, uint property,
        out uint type, byte[] buffer, ref uint length, uint flags);
    [DllImport("cfgmgr32.dll", CharSet = CharSet.Unicode)]
    private static extern uint CM_Get_Device_Interface_ListW(ref Guid kind, string id,
        [Out] char[] buffer, uint length, uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(string path, uint access,
        uint share, IntPtr security, uint creation, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool DeviceIoControl(SafeFileHandle file, uint code,
        byte[] input, uint inputSize, byte[] output, uint outputSize,
        out uint used, IntPtr overlapped);

    public sealed class Property
    {
        public uint Code { get; set; }
        public uint Status { get; set; }
        public string Value { get; set; }
    }

    public sealed class Node
    {
        public string InstanceId { get; set; }
        public Property Class { get; set; }
        public Property Service { get; set; }
        public uint ParentStatus { get; set; }
    }

    public sealed class Evidence
    {
        public Node[] Nodes { get; set; }
        public string StopReason { get; set; }
        public bool InterfaceCorrelated { get; set; }
        public bool ChainRechecked { get; set; }
        public PhysicalEvidence PhysicalTopology { get; set; }
        public bool CompleteBackingGraph { get { return false; } }
        public bool BindingAdmitted { get { return false; } }
    }

    public sealed class PhysicalEvidence
    {
        public string Status { get; set; }
        public uint Win32Error { get; set; }
        public uint ReturnedBytes { get; set; }
        public uint Nodes { get; set; }
        public uint Records { get; set; }
        public string BytesBase64 { get; set; }
    }

    public static PhysicalEvidence DecodePhysical(byte[] bytes, uint used, int budget)
    {
        if (bytes == null || used < 56 || used > bytes.Length || used > 65536 ||
            BitConverter.ToUInt32(bytes, 0) != 56 ||
            BitConverter.ToUInt32(bytes, 4) != used || budget < 1 || budget > 8)
            throw new IOException("Unsupported bounded physical topology header.");
        uint count = BitConverter.ToUInt32(bytes, 8);
        if (count < 1 || count > budget || 16 + count * 40 > used)
            throw new IOException("Physical topology exceeds the approved node count.");
        uint records = count;
        uint tableEnd = 16 + count * 40;
        var starts = new List<uint>();
        var ends = new List<uint>();
        for (int i = 0; i < count; i++)
        {
            int position = 16 + i * 40;
            uint adapters = BitConverter.ToUInt32(bytes, position + 4);
            uint devices = BitConverter.ToUInt32(bytes, position + 16);
            if (adapters > 1 || devices < 1 || devices > budget ||
                records + adapters + devices > budget)
                throw new IOException("Physical topology exceeds the approved record count.");
            records += adapters + devices;
            for (int part = 0; part < 2; part++)
            {
                uint entries = part == 0 ? adapters : devices;
                uint length = BitConverter.ToUInt32(bytes, position + (part == 0 ? 8 : 20));
                uint offset = BitConverter.ToUInt32(bytes, position + (part == 0 ? 12 : 24));
                if (entries == 0)
                {
                    if (length != 0 || offset != 0)
                        throw new IOException("Unexpected empty physical topology extent.");
                }
                else if (length != entries * (part == 0 ? 128u : 136u) ||
                    offset > used - (uint)position || offset < tableEnd - (uint)position ||
                    length > used - (uint)position - offset)
                    throw new IOException("Invalid physical topology data extent.");
                if (entries > 0)
                {
                    uint start = (uint)position + offset, end = start + length;
                    for (int range = 0; range < starts.Count; range++)
                        if (start < ends[range] && starts[range] < end)
                            throw new IOException("Overlapping physical topology data extents.");
                    starts.Add(start);
                    ends.Add(end);
                }
            }
        }
        return new PhysicalEvidence { Status = "bounded_shape_observed_only",
            ReturnedBytes = used, Nodes = count, Records = records,
            BytesBase64 = Convert.ToBase64String(bytes, 0, (int)used) };
    }

    private static PhysicalEvidence ReadPhysical(SafeFileHandle file, int budget)
    {
        if (budget < 2)
            return new PhysicalEvidence { Status = "bounded_refusal_record_budget" };
        byte[] input = new byte[12], output = new byte[65536];
        BitConverter.GetBytes(54u).CopyTo(input, 0);
        uint used;
        if (!DeviceIoControl(file, 0x002d1400, input, 12, output, 65536, out used, IntPtr.Zero))
        {
            uint error = (uint)Marshal.GetLastWin32Error();
            return new PhysicalEvidence { Win32Error = error, ReturnedBytes = used,
                Status = error == 122 || error == 234 ? "bounded_refusal" : "unavailable" };
        }
        return DecodePhysical(output, used, budget);
    }

    private static string Instance(uint node)
    {
        var buffer = new StringBuilder(32768);
        if (CM_Get_Device_IDW(node, buffer, 32768, 0) != 0 || buffer.Length == 0)
            throw new IOException("Device instance ID unavailable within the approved bound.");
        return buffer.ToString();
    }

    public static string DecodeString(uint type, byte[] bytes, uint length)
    {
        if (bytes == null || type != 1 || length < 2 || length > 65536 ||
            length > bytes.Length || length % 2 != 0 ||
            bytes[length - 1] != 0 || bytes[length - 2] != 0)
            throw new IOException("Malformed bounded device string property.");
        string value = new UnicodeEncoding(false, false, true).GetString(bytes, 0, (int)length - 2);
        if (value.IndexOf('\0') >= 0)
            throw new IOException("Embedded null in device string property.");
        return value;
    }

    public static string DecodeInterface(char[] buffer)
    {
        if (buffer == null || buffer.Length < 2 || buffer.Length > 32768)
            throw new IOException("Invalid bounded disk interface response.");
        int end = Array.IndexOf(buffer, '\0');
        if (end < 1 || end + 1 >= buffer.Length || buffer[end + 1] != '\0')
            throw new IOException("Exactly one bounded disk interface is required.");
        return new string(buffer, 0, end);
    }

    private static Property ReadProperty(uint node, uint code)
    {
        byte[] bytes = new byte[65536];
        uint length = 65536, type;
        uint status = CM_Get_DevNode_Registry_PropertyW(node, code, out type, bytes, ref length, 0);
        var result = new Property { Code = code, Status = status };
        if (status != 0) return result;
        result.Value = DecodeString(type, bytes, length);
        return result;
    }

    private static string Interface(string id)
    {
        if (String.IsNullOrEmpty(id) || id.Length >= 32768 || id.IndexOf('\0') >= 0)
            throw new IOException("Invalid bounded device instance ID.");
        Guid disk = new Guid("53f56307-b6bf-11d0-94f2-00a0c91efb8b");
        char[] buffer = new char[32768];
        if (CM_Get_Device_Interface_ListW(ref disk, id, buffer, 32768, 0) != 0)
            throw new IOException("Disk interface unavailable within the approved bound.");
        return DecodeInterface(buffer);
    }

    private static byte[] Query(SafeFileHandle file, uint code, byte[] input)
    {
        byte[] bytes = new byte[65536];
        uint used;
        if (!DeviceIoControl(file, code, input, input == null ? 0u : (uint)input.Length,
            bytes, 65536, out used, IntPtr.Zero))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Bounded interface query failed.");
        if (used == 0 || used > bytes.Length)
            throw new IOException("Invalid bounded interface response.");
        Array.Resize(ref bytes, (int)used);
        return bytes;
    }

    private static void Correlate(SafeFileHandle file, uint disk, byte[] expected)
    {
        byte[] number = Query(file, 0x002d1080, null);
        if (number.Length != 12 || BitConverter.ToUInt32(number, 0) != 7 ||
            BitConverter.ToUInt32(number, 4) != disk)
            throw new IOException("PnP interface does not match the observed C: device.");
        byte[] input = new byte[12];
        BitConverter.GetBytes(2u).CopyTo(input, 0);
        byte[] actual = Query(file, 0x002d1400, input);
        if (expected == null || actual.Length != expected.Length)
            throw new IOException("PnP interface descriptor does not match the C: observation.");
        for (int i = 0; i < actual.Length; i++)
            if (actual[i] != expected[i])
                throw new IOException("PnP interface descriptor changed.");
    }

    public static Evidence Collect(string instanceId, uint disk, byte[] expectedDescriptor)
    {
        uint current;
        if (CM_Locate_DevNodeW(out current, instanceId, 0) != 0)
            throw new IOException("Observed C: device instance is unavailable.");
        string path = Interface(instanceId);
        using (SafeFileHandle file = CreateFileW(path, 0, 3, IntPtr.Zero, 3, 0, IntPtr.Zero))
        {
            if (file.IsInvalid)
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Disk interface open failed.");
            Correlate(file, disk, expectedDescriptor);
            var nodes = new List<Node>();
            var handles = new List<uint>();
            var seen = new HashSet<uint>();
            string stop = "record_bound";
            while (nodes.Count < 8)
            {
                if (!seen.Add(current)) throw new IOException("Device parent cycle.");
                var node = new Node { InstanceId = Instance(current),
                    Class = ReadProperty(current, 8), Service = ReadProperty(current, 5) };
                if (nodes.Count == 0 && !String.Equals(node.InstanceId, instanceId,
                    StringComparison.OrdinalIgnoreCase))
                    throw new IOException("Device instance changed before parent collection.");
                nodes.Add(node);
                handles.Add(current);
                if (node.Class.Status != 0)
                {
                    stop = "class_unavailable";
                    break;
                }
                if (node.Class.Value == "SCSIAdapter" || node.Class.Value == "HDC")
                {
                    stop = "controller_observed";
                    break;
                }
                if (node.Class.Value != "DiskDrive")
                {
                    stop = "unsupported_related_class";
                    break;
                }
                uint parent;
                node.ParentStatus = CM_Get_Parent(out parent, current, 0);
                if (node.ParentStatus != 0)
                {
                    stop = "parent_unavailable";
                    break;
                }
                current = parent;
            }
            for (int i = 0; i < nodes.Count; i++)
            {
                if (Instance(handles[i]) != nodes[i].InstanceId)
                    throw new IOException("Device instance changed during collection.");
                Property deviceClass = ReadProperty(handles[i], 8);
                Property service = ReadProperty(handles[i], 5);
                if (deviceClass.Status != nodes[i].Class.Status ||
                    deviceClass.Value != nodes[i].Class.Value ||
                    service.Status != nodes[i].Service.Status ||
                    service.Value != nodes[i].Service.Value)
                    throw new IOException("Device properties changed during collection.");
                if (i + 1 < nodes.Count)
                {
                    uint parent;
                    if (CM_Get_Parent(out parent, handles[i], 0) != 0 || parent != handles[i + 1])
                        throw new IOException("Device parent relationship changed.");
                }
            }
            if (Interface(instanceId) != path)
                throw new IOException("Disk interface changed during collection.");
            PhysicalEvidence physical = ReadPhysical(file, 8 - nodes.Count);
            Correlate(file, disk, expectedDescriptor);
            PhysicalEvidence rechecked = ReadPhysical(file, 8 - nodes.Count);
            if (physical.Status != rechecked.Status ||
                physical.Win32Error != rechecked.Win32Error ||
                physical.ReturnedBytes != rechecked.ReturnedBytes ||
                physical.BytesBase64 != rechecked.BytesBase64)
                throw new IOException("Physical topology changed during collection.");
            return new Evidence { Nodes = nodes.ToArray(), StopReason = stop,
                InterfaceCorrelated = true, ChainRechecked = true, PhysicalTopology = physical };
        }
    }
}
