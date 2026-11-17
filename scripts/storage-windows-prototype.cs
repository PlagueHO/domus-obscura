// Isolated metadata experiment. This code is not an application filesystem adapter.
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

public static class StorageWindowsPrototype
{
    [StructLayout(LayoutKind.Sequential)]
    private struct UnicodeString
    {
        public ushort Length;
        public ushort MaximumLength;
        public IntPtr Buffer;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ObjectAttributes
    {
        public uint Length;
        public IntPtr RootDirectory;
        public IntPtr ObjectName;
        public uint Attributes;
        public IntPtr SecurityDescriptor;
        public IntPtr SecurityQualityOfService;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct IoStatus
    {
        public IntPtr Status;
        public UIntPtr Information;
    }

    [DllImport("ntdll.dll")]
    private static extern int NtCreateFile(out SafeFileHandle file, uint access,
        ref ObjectAttributes attributes, out IoStatus status, IntPtr allocation,
        uint fileAttributes, uint share, uint disposition, uint options,
        IntPtr ea, uint eaLength);

    [DllImport("ntdll.dll")]
    private static extern int NtQueryVolumeInformationFile(SafeFileHandle file,
        out IoStatus status, byte[] buffer, uint length, uint informationClass);

    [DllImport("ntdll.dll")]
    private static extern uint RtlNtStatusToDosError(int status);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(string path, uint access,
        uint share, IntPtr security, uint disposition, uint flags, IntPtr template);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandleEx(SafeFileHandle file,
        int informationClass, byte[] buffer, uint length);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetFinalPathNameByHandleW(SafeFileHandle file,
        StringBuilder path, uint length, uint flags);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool GetVolumeInformationByHandleW(SafeFileHandle file,
        StringBuilder label, uint labelSize, out uint serial, out uint maxComponent,
        out uint flags, StringBuilder filesystem, uint filesystemSize);

    public sealed class Evidence
    {
        public string Identity { get; set; }
        public string Coordinate { get; set; }
        public string Filesystem { get; set; }
        public bool Remote { get; set; }
    }

    public sealed class Root : IDisposable
    {
        private readonly List<SafeFileHandle> handles;
        public string Path { get; private set; }
        public Evidence Evidence { get; private set; }
        public string[] Chain { get; private set; }
        internal SafeFileHandle RetainedHandle { get { return handles[handles.Count - 1]; } }

        internal Root(string path, List<SafeFileHandle> retained)
        {
            Path = path;
            handles = retained;
            Chain = handles.Select(Identity).ToArray();
            Evidence = Observe(handles[handles.Count - 1]);
        }

        public Evidence ReadRetained()
        {
            return Observe(handles[handles.Count - 1]);
        }

        public void Recheck()
        {
            using (Root current = Open(Path))
            {
                Evidence retained = ReadRetained();
                if (!Chain.SequenceEqual(current.Chain)
                    || Evidence.Identity != retained.Identity
                    || Evidence.Coordinate != retained.Coordinate
                    || Evidence.Coordinate != current.Evidence.Coordinate
                    || Evidence.Filesystem != retained.Filesystem
                    || Evidence.Filesystem != current.Evidence.Filesystem
                    || Evidence.Remote != retained.Remote
                    || Evidence.Remote != current.Evidence.Remote)
                    throw new IOException("Retained root or configured path changed.");
            }
        }

        public Evidence Child(string name)
        {
            using (SafeFileHandle child = OpenChild(handles[handles.Count - 1], name))
            {
                Evidence result = Observe(child);
                if (Relationship(Evidence, result) != "Overlapping")
                    throw new IOException("Child left the verified volume coordinate.");
                Recheck();
                return result;
            }
        }

        public void Dispose()
        {
            for (int index = handles.Count - 1; index >= 0; index--)
                handles[index].Dispose();
        }
    }

    private static void RequireHandle(SafeFileHandle file)
    {
        if (file.IsInvalid)
        {
            int error = Marshal.GetLastWin32Error();
            file.Dispose();
            throw new Win32Exception(error);
        }
    }

    private static byte[] Information(SafeFileHandle file, int kind, int size)
    {
        byte[] bytes = new byte[size];
        if (!GetFileInformationByHandleEx(file, kind, bytes, (uint)size))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        return bytes;
    }

    private static void RequireDirectory(SafeFileHandle file)
    {
        byte[] tag = Information(file, 9, 8); // FileAttributeTagInfo
        uint attributes = BitConverter.ToUInt32(tag, 0);
        if ((attributes & 0x400) != 0 || (attributes & 0x10) == 0)
            throw new IOException("Reparse points and non-directories are rejected.");
    }

    private static string Identity(SafeFileHandle file)
    {
        byte[] id = Information(file, 18, 24); // FileIdInfo: full 128-bit ID only
        return "win128:" + BitConverter.ToUInt64(id, 0).ToString("x16") + ":"
            + String.Concat(id.Skip(8).Reverse().Select(value => value.ToString("x2")));
    }

    private static Evidence Observe(SafeFileHandle file)
    {
        RequireDirectory(file);
        byte[] device = new byte[8];
        IoStatus status;
        int code = NtQueryVolumeInformationFile(file, out status, device, 8, 4);
        if (code != 0)
            throw new Win32Exception((int)RtlNtStatusToDosError(code));
        if (status.Information.ToUInt64() != 8)
            throw new IOException("Incomplete device information.");
        bool remote = (BitConverter.ToUInt32(device, 4) & 0x10) != 0;
        if (remote)
            throw new IOException("Remote filesystem relationship is unknown; no local-path fallback.");
        StringBuilder filesystem = new StringBuilder(64);
        uint serial, maxComponent, flags;
        if (!GetVolumeInformationByHandleW(file, null, 0, out serial,
            out maxComponent, out flags, filesystem, 64))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        StringBuilder coordinate = new StringBuilder(32769);
        uint count = GetFinalPathNameByHandleW(file, coordinate, 32769, 1);
        if (count == 0)
            throw new Win32Exception(Marshal.GetLastWin32Error());
        if (count >= 32769 || count != coordinate.Length)
            throw new IOException("Unbounded or incomplete normalized volume coordinate.");
        return new Evidence {
            Identity = Identity(file), Coordinate = coordinate.ToString(),
            Filesystem = filesystem.ToString(), Remote = remote
        };
    }

    private static SafeFileHandle OpenChild(SafeFileHandle parent, string name)
    {
        if (String.IsNullOrEmpty(name) || name.Length > 255
            || name == "." || name == ".." || name.IndexOfAny(new[] { '\\', '/', ':', '\0' }) >= 0)
            throw new IOException("Invalid single directory component.");
        IntPtr text = Marshal.StringToHGlobalUni(name);
        IntPtr unicodePointer = IntPtr.Zero;
        try
        {
            UnicodeString unicode = new UnicodeString {
                Length = checked((ushort)(name.Length * 2)),
                MaximumLength = checked((ushort)((name.Length + 1) * 2)), Buffer = text
            };
            unicodePointer = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(UnicodeString)));
            Marshal.StructureToPtr(unicode, unicodePointer, false);
            ObjectAttributes attributes = new ObjectAttributes {
                Length = (uint)Marshal.SizeOf(typeof(ObjectAttributes)),
                RootDirectory = parent.DangerousGetHandle(), ObjectName = unicodePointer,
                Attributes = 0x40
            };
            SafeFileHandle child;
            IoStatus status;
            // Retained parent-relative open, existing directory, synchronous,
            // and no reparse processing. No file creation or backup privileges.
            int code = NtCreateFile(out child, 0x1000a0, ref attributes, out status,
                IntPtr.Zero, 0, 7, 1, 0x200021, IntPtr.Zero, 0);
            GC.KeepAlive(parent);
            if (code != 0)
            {
                if (child != null) child.Dispose();
                throw new Win32Exception((int)RtlNtStatusToDosError(code));
            }
            try { RequireDirectory(child); return child; }
            catch { child.Dispose(); throw; }
        }
        finally
        {
            if (unicodePointer != IntPtr.Zero) Marshal.FreeHGlobal(unicodePointer);
            Marshal.FreeHGlobal(text);
        }
    }

    public static Root Open(string path)
    {
        string anchor;
        string tail;
        if (path.Length > 32768) throw new IOException("Path exceeds prototype bound.");
        if (path.StartsWith(@"\\?\Volume{", StringComparison.OrdinalIgnoreCase))
        {
            int close = path.IndexOf(@"}\", StringComparison.Ordinal);
            Guid volume;
            if (close != 47 || !Guid.TryParse(path.Substring(11, 36), out volume))
                throw new IOException("Invalid volume GUID anchor.");
            anchor = path.Substring(0, 49);
            tail = path.Substring(49);
        }
        else
        {
            if (path.Length < 3 || !((path[0] >= 'A' && path[0] <= 'Z')
                || (path[0] >= 'a' && path[0] <= 'z')) || path[1] != ':' || path[2] != '\\')
                throw new IOException("Only an absolute drive or volume GUID path is accepted.");
            anchor = path.Substring(0, 3);
            tail = path.Substring(3);
        }
        string[] components = tail.Length == 0 ? new string[0] : tail.Split('\\');
        if (components.Length > 256)
            throw new IOException("Too many path components.");
        List<SafeFileHandle> handles = new List<SafeFileHandle>();
        try
        {
            SafeFileHandle first = CreateFileW(anchor, 0x1000a0, 7, IntPtr.Zero, 3,
                0x02200000, IntPtr.Zero);
            RequireHandle(first);
            handles.Add(first);
            RequireDirectory(first);
            // Remote device queries precede all child traversal, including mapped drives.
            Observe(first);
            foreach (string component in components)
                handles.Add(OpenChild(handles[handles.Count - 1], component));
            return new Root(path, handles);
        }
        catch
        {
            foreach (SafeFileHandle file in handles) file.Dispose();
            throw;
        }
    }

    private static bool Coordinate(string value, out string volume, out string[] parts)
    {
        volume = null;
        parts = null;
        if (value == null || value.Length > 32768 || !value.StartsWith(@"\\?\Volume{"))
            return false;
        int close = value.IndexOf(@"}\", StringComparison.Ordinal);
        Guid guid;
        if (close != 47 || !Guid.TryParse(value.Substring(11, 36), out guid))
            return false;
        volume = guid.ToString();
        string tail = value.Substring(49);
        parts = tail.Length == 0 ? new string[0] : tail.Split('\\');
        return parts.Length <= 256 && parts.All(part => part.Length > 0
            && part != "." && part != ".." && part.All(character => character >= 32 && character <= 126)
            && part.IndexOfAny(new[] { ':', '/' }) < 0);
    }

    private static bool Nested(string[] child, string[] parent)
    {
        return child.Length >= parent.Length && parent.Select((part, index) =>
            String.Equals(part, child[index], StringComparison.OrdinalIgnoreCase)).All(equal => equal);
    }

    public static string Relationship(Evidence left, Evidence right)
    {
        if (left == null || right == null || !ValidIdentity(left.Identity) || !ValidIdentity(right.Identity))
            return "UnknownIdentity";
        // Remote server/share spellings, filesystem names and serial values
        // cannot establish a common backing namespace.
        if (left.Remote || right.Remote) return "UnknownRemote";
        if (left.Filesystem != "NTFS" || right.Filesystem != "NTFS")
            return "UnknownFilesystem";
        string leftVolume, rightVolume;
        string[] leftParts, rightParts;
        if (!Coordinate(left.Coordinate, out leftVolume, out leftParts)
            || !Coordinate(right.Coordinate, out rightVolume, out rightParts))
            return "UnknownCoordinate";
        if (leftVolume != rightVolume) return "UnknownVolume";
        if (left.Identity == right.Identity || Nested(leftParts, rightParts) || Nested(rightParts, leftParts))
            return "Overlapping";
        return "Separated";
    }

    private static bool ValidIdentity(string value)
    {
        return value != null && value.Length == 56 && value.StartsWith("win128:")
            && value[23] == ':' && value.Skip(7).Where((character, index) => index != 16)
                .All(character => (character >= '0' && character <= '9')
                    || (character >= 'a' && character <= 'f'));
    }
}
