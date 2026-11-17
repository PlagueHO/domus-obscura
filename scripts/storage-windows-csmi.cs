// Metadata-only diagnostic. Wire constants checked against Windows SDK and
// smartmontools csmisas.h at a3aee76ed7f342efebd5f01631be1d79f7fdb780.
// No pass-through, controller mutation or generic miniport command API.
// The separate RAID candidate path requires the previously observed driver.
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

public static class StorageWindowsCsmi
{
    [DllImport("cfgmgr32.dll", CharSet = CharSet.Unicode)]
    private static extern uint CM_Locate_DevNodeW(out uint node, string id, uint flags);
    [DllImport("cfgmgr32.dll")]
    private static extern uint CM_Get_Parent(out uint parent, uint node, uint flags);
    [DllImport("cfgmgr32.dll", CharSet = CharSet.Unicode)]
    private static extern uint CM_Get_Device_IDW(uint node, StringBuilder id, uint length, uint flags);
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

    public sealed class CommandEvidence
    {
        public uint Command { get; set; }
        public string Status { get; set; }
        public uint Win32Error { get; set; }
        public uint ReturnCode { get; set; }
        public uint ReturnedBytes { get; set; }
        public string BytesBase64 { get; set; }
        public string Failure { get; set; }
    }

    public sealed class Evidence
    {
        public string Status { get; set; }
        public uint ConfigurationStatus { get; set; }
        public uint OpenError { get; set; }
        public bool ControllerCorrelated { get; set; }
        public bool Rechecked { get; set; }
        public CommandEvidence[] Commands { get; set; }
        public int CandidateMembers { get; set; }
        public bool BindingAdmitted { get { return false; } }
    }

    private static int FrameSize(uint command)
    {
        switch (command)
        {
            case 1: return 204;
            case 10: return 128;
            case 20: return 2080;
            default: throw new IOException("Command is outside the approved metadata allowlist.");
        }
    }

    public static byte[] Frame(uint command)
    {
        var bytes = new byte[FrameSize(command)];
        BitConverter.GetBytes(28u).CopyTo(bytes, 0);
        Encoding.ASCII.GetBytes(command == 1 ? "CSMIALL" :
            command == 10 ? "CSMIARY" : "CSMISAS").CopyTo(bytes, 4);
        BitConverter.GetBytes(5u).CopyTo(bytes, 12);
        BitConverter.GetBytes(command).CopyTo(bytes, 16);
        BitConverter.GetBytes(UInt32.MaxValue).CopyTo(bytes, 20);
        BitConverter.GetBytes((uint)bytes.Length - 28).CopyTo(bytes, 24);
        return bytes;
    }

    public static uint Decode(uint command, byte[] bytes, uint used)
    {
        byte[] expected = Frame(command);
        if (bytes == null || used != expected.Length || bytes.Length < used ||
            BitConverter.ToUInt32(bytes, 0) != 28 ||
            BitConverter.ToUInt32(bytes, 16) != command ||
            BitConverter.ToUInt32(bytes, 24) != used - 28)
            throw new IOException("Malformed bounded CSMI response header.");
        for (int i = 4; i < 12; i++)
            if (bytes[i] != expected[i]) throw new IOException("CSMI response signature mismatch.");
        uint result = BitConverter.ToUInt32(bytes, 20);
        if (result == UInt32.MaxValue)
            throw new IOException("CSMI response did not replace the request status.");
        if (result != 0) return result;
        if (command == 1 && (Array.IndexOf(bytes, (byte)0, 28, 81) < 0 ||
            Array.IndexOf(bytes, (byte)0, 109, 81) < 0))
            throw new IOException("Unterminated CSMI driver information.");
        if (command == 10 && BitConverter.ToUInt32(bytes, 28) > 8)
            throw new IOException("CSMI RAID count exceeds the approved bound.");
        if (command == 20)
        {
            int count = bytes[28];
            // Two summary records plus at most six PHY records fit the approval.
            if (count > 6) throw new IOException("CSMI PHY count exceeds the approved record budget.");
            for (int i = 32 + count * 64; i < 2080; i++)
                if (bytes[i] != 0)
                    throw new IOException("CSMI PHY data extends beyond its declared count.");
        }
        return 0;
    }

    public static byte[] RaidFrame(int capacity)
    {
        if (capacity < 1 || capacity > 6)
            throw new IOException("RAID candidate capacity exceeds the approved bound.");
        var bytes = new byte[64 + capacity * 128];
        BitConverter.GetBytes(28u).CopyTo(bytes, 0);
        Encoding.ASCII.GetBytes("CSMIARY").CopyTo(bytes, 4);
        BitConverter.GetBytes(5u).CopyTo(bytes, 12);
        BitConverter.GetBytes(11u).CopyTo(bytes, 16);
        BitConverter.GetBytes(UInt32.MaxValue).CopyTo(bytes, 20);
        BitConverter.GetBytes((uint)bytes.Length - 28).CopyTo(bytes, 24);
        return bytes;
    }

    private static void EqualResponse(byte[] actual, byte[] expected)
    {
        if (actual == null || expected == null || actual.Length != expected.Length)
            throw new IOException("The previously observed controller metadata changed.");
        for (int i = 0; i < actual.Length; i++)
            if (actual[i] != expected[i])
                throw new IOException("The previously observed controller metadata changed.");
    }

    public static int RaidPrerequisites(byte[] driver, byte[] raid,
        byte[] expectedDriver, byte[] expectedRaid)
    {
        if (driver == null || raid == null ||
            Decode(1, driver, (uint)driver.Length) != 0 ||
            Decode(10, raid, (uint)raid.Length) != 0 ||
            BitConverter.ToUInt16(driver, 198) != 0 ||
            BitConverter.ToUInt16(driver, 200) != 81 ||
            BitConverter.ToUInt32(raid, 28) != 1)
            throw new IOException("The approved historical RAID profile is unavailable.");
        EqualResponse(driver, expectedDriver);
        EqualResponse(raid, expectedRaid);
        uint capacity = BitConverter.ToUInt32(raid, 32);
        if (capacity < 1 || capacity > 6)
            throw new IOException("Reported RAID capacity exceeds the approved bound.");
        return (int)capacity;
    }

    private static void ProveParent(string disk, string controller)
    {
        if (String.IsNullOrEmpty(disk) || String.IsNullOrEmpty(controller) ||
            disk.Length >= 32768 || controller.Length >= 32768 ||
            disk.IndexOf('\0') >= 0 || controller.IndexOf('\0') >= 0)
            throw new IOException("Invalid bounded device ancestry input.");
        uint child, parent;
        if (CM_Locate_DevNodeW(out child, disk, 0) != 0 ||
            CM_Get_Parent(out parent, child, 0) != 0)
            throw new IOException("Actual C: controller ancestry is unavailable.");
        var id = new StringBuilder(32768);
        if (CM_Get_Device_IDW(parent, id, 32768, 0) != 0 ||
            !String.Equals(id.ToString(), controller, StringComparison.OrdinalIgnoreCase))
            throw new IOException("Controller is not the observed C: device's direct parent.");
    }

    private static string Interface(string id, Guid kind, out uint status)
    {
        char[] bytes = new char[32768];
        status = CM_Get_Device_Interface_ListW(ref kind, id, bytes, 32768, 0);
        if (status != 0 || bytes[0] == '\0') return null;
        return StorageWindowsTopology.DecodeInterface(bytes);
    }

    private static void CorrelateDisk(SafeFileHandle file, uint disk, byte[] descriptor)
    {
        foreach (uint property in new uint[] { UInt32.MaxValue, 2 })
        {
            byte[] input = property == UInt32.MaxValue ? null : new byte[12];
            if (input != null) BitConverter.GetBytes(property).CopyTo(input, 0);
            byte[] bytes = new byte[65536];
            uint used;
            if (!DeviceIoControl(file, input == null ? 0x002d1080u : 0x002d1400u,
                input, input == null ? 0u : 12u, bytes, 65536, out used, IntPtr.Zero))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "C: interface recheck failed.");
            if (input == null)
            {
                if (used != 12 || BitConverter.ToUInt32(bytes, 0) != 7 ||
                    BitConverter.ToUInt32(bytes, 4) != disk)
                    throw new IOException("Disk interface number does not match C:.");
            }
            else
            {
                if (descriptor == null || used != descriptor.Length)
                    throw new IOException("Disk interface descriptor size changed.");
                for (int i = 0; i < descriptor.Length; i++)
                    if (bytes[i] != descriptor[i])
                        throw new IOException("Disk interface descriptor changed.");
            }
        }
    }

    public static Evidence Collect(string diskId, string controllerId, uint disk, byte[] descriptor)
    {
        return CollectSelected(diskId, controllerId, disk, descriptor, null, null);
    }

    public static Evidence CollectRaid(string diskId, string controllerId, uint disk,
        byte[] descriptor, byte[] expectedDriver, byte[] expectedRaid)
    {
        RaidPrerequisites(expectedDriver, expectedRaid, expectedDriver, expectedRaid);
        return CollectSelected(diskId, controllerId, disk, descriptor, expectedDriver, expectedRaid);
    }

    private static Evidence CollectSelected(string diskId, string controllerId, uint disk,
        byte[] descriptor, byte[] expectedDriver, byte[] expectedRaid)
    {
        bool raidCandidate = expectedDriver != null;
        int capacity = raidCandidate ? RaidPrerequisites(expectedDriver, expectedRaid,
            expectedDriver, expectedRaid) : 0;
        int members = 0;
        byte[] observedDriver = null;
        ProveParent(diskId, controllerId);
        uint status;
        string diskPath = Interface(diskId,
            new Guid("53f56307-b6bf-11d0-94f2-00a0c91efb8b"), out status);
        if (diskPath == null) return new Evidence { Status = "disk_interface_unavailable",
            ConfigurationStatus = status };
        using (SafeFileHandle child = CreateFileW(diskPath, 0, 3, IntPtr.Zero, 3, 0, IntPtr.Zero))
        {
            if (child.IsInvalid)
                return new Evidence { Status = "disk_open_failed",
                    OpenError = (uint)Marshal.GetLastWin32Error() };
            CorrelateDisk(child, disk, descriptor);
            string path = Interface(controllerId,
                new Guid("2accfe60-c130-11d2-b082-00a0c91efb8b"), out status);
            if (path == null) return new Evidence { Status = "controller_interface_unavailable",
                ConfigurationStatus = status };
            ProveParent(diskId, controllerId);
            using (SafeFileHandle controller = CreateFileW(path, 0xc0000000, 3,
                IntPtr.Zero, 3, 0, IntPtr.Zero))
            {
                if (controller.IsInvalid)
                    return new Evidence { Status = "controller_open_failed",
                        OpenError = (uint)Marshal.GetLastWin32Error(), ControllerCorrelated = true };
                var commands = new List<CommandEvidence>();
                foreach (uint command in new uint[] { 1, 10, raidCandidate ? 11u : 20u })
                {
                    ProveParent(diskId, controllerId);
                    CorrelateDisk(child, disk, descriptor);
                    byte[] input = command == 11 ? RaidFrame(capacity) : Frame(command);
                    byte[] output = new byte[input.Length];
                    uint used;
                    bool ok = DeviceIoControl(controller, 0x0004d008, input,
                        (uint)input.Length, output, (uint)output.Length, out used, IntPtr.Zero);
                    uint error = ok ? 0u : (uint)Marshal.GetLastWin32Error();
                    var observed = new CommandEvidence { Command = command,
                        ReturnedBytes = used, Win32Error = error,
                        ReturnCode = UInt32.MaxValue,
                        Status = ok ? "metadata_observed" : "native_query_failed" };
                    commands.Add(observed);
                    if (!ok) break;
                    if (used > output.Length)
                        throw new IOException("Native response exceeds the supplied buffer.");
                    observed.BytesBase64 = Convert.ToBase64String(output, 0, (int)used);
                    if (used >= 24) observed.ReturnCode = BitConverter.ToUInt32(output, 20);
                    try
                    {
                        if (command == 11)
                        {
                            members = StorageWindowsCsmiRaid.Decode(output, used, 0, capacity, 0, 81);
                            observed.ReturnCode = 0;
                        }
                        else observed.ReturnCode = Decode(command, output, used);
                        observed.Status = observed.ReturnCode == 0 ? "metadata_observed" : "driver_refused";
                        if (observed.ReturnCode == 0 && raidCandidate && command == 1)
                        {
                            EqualResponse(output, expectedDriver);
                            observedDriver = output;
                        }
                        if (observed.ReturnCode == 0 && raidCandidate && command == 10)
                            RaidPrerequisites(observedDriver, output, expectedDriver, expectedRaid);
                    }
                    catch (IOException errorResponse)
                    {
                        observed.Failure = errorResponse.Message;
                        observed.Status = "response_refused";
                        break;
                    }
                    catch (NotSupportedException unsupported)
                    {
                        observed.Failure = unsupported.Message;
                        observed.Status = "candidate_layout_refused";
                        break;
                    }
                    if (observed.ReturnCode != 0) break;
                }
                ProveParent(diskId, controllerId);
                CorrelateDisk(child, disk, descriptor);
                if (Interface(controllerId, new Guid("2accfe60-c130-11d2-b082-00a0c91efb8b"),
                    out status) != path || status != 0)
                    throw new IOException("Controller interface changed during the probe.");
                return new Evidence { Status = commands.Count == 3 &&
                    commands[2].Status == "metadata_observed" ?
                    (raidCandidate ? "raid_candidate_completed" : "metadata_only_completed") : "query_blocked",
                    Commands = commands.ToArray(), CandidateMembers = members,
                    ControllerCorrelated = true, Rechecked = true };
            }
        }
    }
}
