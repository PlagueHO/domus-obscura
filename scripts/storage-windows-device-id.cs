// Offline diagnostic. A descriptor match is not production backing authority.
using System;
using System.Collections.Generic;
using System.IO;

public static class StorageWindowsDeviceId
{
    public const int MaximumIdentifiers = 8;
    public const int MaximumResponse = 65536;

    private static List<byte[]> Decode(byte[] bytes)
    {
        if (bytes == null || bytes.Length < 12 || bytes.Length > MaximumResponse)
            throw new IOException("Incomplete or oversized device-ID descriptor.");
        if (BitConverter.ToUInt32(bytes, 0) != 16)
            throw new NotSupportedException("Unsupported device-ID descriptor version.");
        if (BitConverter.ToUInt32(bytes, 4) != bytes.Length)
            throw new IOException("Device-ID descriptor size does not match its response.");
        uint count = BitConverter.ToUInt32(bytes, 8);
        if (count == 0 || count > MaximumIdentifiers)
            throw new IOException("Unsupported device-ID descriptor count.");
        var supported = new List<byte[]>();
        int position = 12;
        for (uint i = 0; i < count; i++)
        {
            if (position > bytes.Length - 16)
                throw new IOException("Truncated identifier header.");
            uint codeSet = BitConverter.ToUInt32(bytes, position);
            uint type = BitConverter.ToUInt32(bytes, position + 4);
            int length = BitConverter.ToUInt16(bytes, position + 8);
            int next = BitConverter.ToUInt16(bytes, position + 10);
            uint association = BitConverter.ToUInt32(bytes, position + 12);
            if (codeSet < 1 || codeSet > 3 || type > 8 || association > 2)
                throw new NotSupportedException("Unknown identifier enum value.");
            int payload = position + 16;
            if (length == 0 || length > bytes.Length - payload)
                throw new IOException("Truncated or empty identifier payload.");
            if (i + 1 < count)
            {
                if (next < 16 + length || next % 4 != 0 ||
                    next > bytes.Length - position - 16)
                    throw new IOException("Invalid next identifier offset.");
            }
            else if ((next != 0 && next != bytes.Length - position) ||
                bytes.Length - payload - length > 4)
                throw new IOException("Invalid final identifier extent.");

            if (codeSet == 1 && type == 2 && association == 0 &&
                (length == 8 || length == 16))
            {
                var identifier = new byte[length];
                Array.Copy(bytes, payload, identifier, 0, length);
                bool zero = true, ones = true;
                foreach (byte value in identifier)
                {
                    zero &= value == 0;
                    ones &= value == 255;
                }
                if (zero || ones)
                    throw new NotSupportedException("Empty or sentinel EUI-based identifier.");
                foreach (byte[] prior in supported)
                    if (Equal(prior, identifier))
                        throw new IOException("Duplicate supported identifier.");
                supported.Add(identifier);
            }
            position += next;
        }
        if (supported.Count == 0)
            throw new NotSupportedException("No supported binary device-associated EUI-based identifier.");
        return supported;
    }

    private static bool Equal(byte[] left, byte[] right)
    {
        if (left.Length != right.Length) return false;
        for (int i = 0; i < left.Length; i++)
            if (left[i] != right[i]) return false;
        return true;
    }

    public static int SupportedCount(byte[] descriptor)
    {
        return Decode(descriptor).Count;
    }

    public static bool Matches(byte[] expected, byte[] observed)
    {
        List<byte[]> left = Decode(expected), right = Decode(observed);
        if (left.Count != right.Count) return false;
        foreach (byte[] identifier in left)
        {
            bool found = false;
            foreach (byte[] candidate in right) found |= Equal(identifier, candidate);
            if (!found) return false;
        }
        return true;
    }
}
