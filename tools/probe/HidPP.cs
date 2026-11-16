// Logitech HID++ 2.0 battery probe. Read-only queries, no admin needed.
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

public class HidPPResult
{
    public string Interface = "";
    public string Product = "";
    public string Path = "";
    public int Index;
    public string Name = "";
    public string Kind = "";
    public string Battery = "";
    public string Source = "";
    public string Notes = "";
}

public static class HidPP
{
    const byte SW = 0x0A; // our software id, distinguishes our replies from G HUB / Options+

    [StructLayout(LayoutKind.Sequential)]
    struct HIDD_ATTRIBUTES { public int Size; public ushort VendorID; public ushort ProductID; public ushort VersionNumber; }

    [StructLayout(LayoutKind.Sequential)]
    struct HIDP_CAPS
    {
        public ushort Usage, UsagePage, InputReportByteLength, OutputReportByteLength, FeatureReportByteLength;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 27)] public ushort[] Rest;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct SP_DEVICE_INTERFACE_DATA { public int cbSize; public Guid InterfaceClassGuid; public int Flags; public IntPtr Reserved; }

    [DllImport("hid.dll")] static extern void HidD_GetHidGuid(out Guid g);
    [DllImport("hid.dll")] static extern bool HidD_GetAttributes(SafeFileHandle h, ref HIDD_ATTRIBUTES a);
    [DllImport("hid.dll")] static extern bool HidD_GetPreparsedData(SafeFileHandle h, out IntPtr p);
    [DllImport("hid.dll")] static extern bool HidD_FreePreparsedData(IntPtr p);
    [DllImport("hid.dll")] static extern int HidP_GetCaps(IntPtr p, ref HIDP_CAPS c);
    [DllImport("hid.dll", CharSet = CharSet.Unicode)] static extern bool HidD_GetProductString(SafeFileHandle h, StringBuilder sb, int len);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode)] static extern IntPtr SetupDiGetClassDevs(ref Guid g, IntPtr e, IntPtr w, int flags);
    [DllImport("setupapi.dll")] static extern bool SetupDiEnumDeviceInterfaces(IntPtr set, IntPtr dev, ref Guid g, int idx, ref SP_DEVICE_INTERFACE_DATA d);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode)] static extern bool SetupDiGetDeviceInterfaceDetail(IntPtr set, ref SP_DEVICE_INTERFACE_DATA d, IntPtr detail, int size, out int req, IntPtr info);
    [DllImport("setupapi.dll")] static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern SafeFileHandle CreateFile(string name, uint access, uint share, IntPtr sa, uint disp, uint flags, IntPtr tmpl);

    static List<string> HidPaths()
    {
        var list = new List<string>();
        Guid g; HidD_GetHidGuid(out g);
        IntPtr set = SetupDiGetClassDevs(ref g, IntPtr.Zero, IntPtr.Zero, 0x12); // PRESENT | DEVICEINTERFACE
        try
        {
            for (int i = 0; ; i++)
            {
                var d = new SP_DEVICE_INTERFACE_DATA(); d.cbSize = Marshal.SizeOf(d);
                if (!SetupDiEnumDeviceInterfaces(set, IntPtr.Zero, ref g, i, ref d)) break;
                int req; SetupDiGetDeviceInterfaceDetail(set, ref d, IntPtr.Zero, 0, out req, IntPtr.Zero);
                IntPtr buf = Marshal.AllocHGlobal(req);
                try
                {
                    Marshal.WriteInt32(buf, IntPtr.Size == 8 ? 8 : 6);
                    if (SetupDiGetDeviceInterfaceDetail(set, ref d, buf, req, out req, IntPtr.Zero))
                        list.Add(Marshal.PtrToStringUni(buf + 4));
                }
                finally { Marshal.FreeHGlobal(buf); }
            }
        }
        finally { SetupDiDestroyDeviceInfoList(set); }
        return list;
    }

    class Channel : IDisposable
    {
        public FileStream Fs; public int InLen, OutLen;
        public BlockingCollection<byte[]> Q = new BlockingCollection<byte[]>();
        volatile bool closed;
        public Channel(SafeFileHandle h, int inLen, int outLen)
        {
            InLen = inLen; OutLen = outLen;
            Fs = new FileStream(h, FileAccess.ReadWrite, 1, true);
            Task.Run(async () =>
            {
                try
                {
                    while (!closed)
                    {
                        var b = new byte[InLen];
                        int n = await Fs.ReadAsync(b, 0, InLen);
                        if (n <= 0) break;
                        Q.Add(b);
                    }
                }
                catch { }
            });
        }
        public void Dispose() { closed = true; try { Fs.Dispose(); } catch { } }
    }

    // Returns params (bytes 4..19) of the reply, or null. err gets the HID++ error code when one is returned.
    static byte[] Request(Channel c, byte dev, byte feat, byte fn, int timeoutMs, out int err, params byte[] p)
    {
        err = -1;
        var msg = new byte[c.OutLen];
        msg[0] = 0x11; msg[1] = dev; msg[2] = feat; msg[3] = (byte)((fn << 4) | SW);
        for (int i = 0; i < p.Length && 4 + i < msg.Length; i++) msg[4 + i] = p[i];
        byte[] junk; while (c.Q.TryTake(out junk)) { }
        try { c.Fs.Write(msg, 0, msg.Length); c.Fs.Flush(); } catch { return null; }
        var deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
        while (true)
        {
            int left = (int)(deadline - DateTime.UtcNow).TotalMilliseconds;
            if (left <= 0) return null;
            byte[] r;
            if (!c.Q.TryTake(out r, left)) return null;
            if (r[0] != 0x11 || r[1] != dev) continue;
            if (r[2] == feat && r[3] == msg[3]) { var o = new byte[r.Length - 4]; Array.Copy(r, 4, o, 0, o.Length); return o; }
            if (r[2] == 0xFF && r[3] == feat && r[4] == msg[3]) { err = r[5]; return null; }
        }
    }

    static int FeatureIndex(Channel c, byte dev, ushort featureId)
    {
        int err;
        var r = Request(c, dev, 0x00, 0, 400, out err, (byte)(featureId >> 8), (byte)featureId);
        if (r == null || r[0] == 0) return -1;
        return r[0];
    }

    static readonly string[] Kinds = { "keyboard", "remote", "numpad", "mouse", "touchpad", "trackball", "presenter", "receiver", "headset", "webcam", "steering wheel", "joystick", "gamepad", "dock", "speaker", "microphone", "illumination", "programmable controller", "car sim pedals", "adapter" };

    // Rough Li-ion discharge curve (mV -> %), only for devices that report voltage instead of percent.
    static int VoltageToPercent(int mv)
    {
        int[,] curve = { { 4186, 100 }, { 4067, 90 }, { 3989, 80 }, { 3922, 70 }, { 3859, 60 }, { 3811, 50 }, { 3778, 40 }, { 3751, 30 }, { 3717, 20 }, { 3671, 10 }, { 3500, 0 } };
        if (mv >= curve[0, 0]) return 100;
        for (int i = 1; i < curve.GetLength(0); i++)
            if (mv >= curve[i, 0])
            {
                double t = (double)(mv - curve[i, 0]) / (curve[i - 1, 0] - curve[i, 0]);
                return (int)Math.Round(curve[i, 1] + t * (curve[i - 1, 1] - curve[i, 1]));
            }
        return 0;
    }

    static HidPPResult ProbeDevice(Channel c, byte dev)
    {
        int err;
        var ping = Request(c, dev, 0x00, 1, 400, out err, 0, 0, 0x5A);
        if (ping == null) return null; // not present, offline, asleep, or HID++ 1.0 only
        var res = new HidPPResult { Index = dev };

        int fi = FeatureIndex(c, dev, 0x0005);
        if (fi > 0)
        {
            var cnt = Request(c, dev, (byte)fi, 0, 400, out err);
            if (cnt != null)
            {
                int len = cnt[0]; var sb = new StringBuilder();
                while (sb.Length < len)
                {
                    var part = Request(c, dev, (byte)fi, 1, 400, out err, (byte)sb.Length);
                    if (part == null) break;
                    for (int i = 0; i < part.Length && sb.Length < len; i++) sb.Append((char)part[i]);
                }
                res.Name = sb.ToString().TrimEnd('\0');
            }
            var t = Request(c, dev, (byte)fi, 2, 400, out err);
            if (t != null) res.Kind = t[0] < Kinds.Length ? Kinds[t[0]] : ("type " + t[0]);
        }

        // 0x1004 Unified Battery
        fi = FeatureIndex(c, dev, 0x1004);
        if (fi > 0)
        {
            var s = Request(c, dev, (byte)fi, 1, 400, out err);
            if (s != null)
            {
                string lvl = (s[1] & 8) != 0 ? "full" : (s[1] & 4) != 0 ? "good" : (s[1] & 2) != 0 ? "low" : (s[1] & 1) != 0 ? "critical" : "?";
                string chg = s[2] == 0 ? "" : s[2] == 1 || s[2] == 2 ? " (charging)" : s[2] == 3 ? " (charged)" : " (charge state " + s[2] + ")";
                res.Battery = (s[0] > 0 ? s[0] + "%" : lvl) + chg;
                res.Source = "HID++ 0x1004 unified battery";
                return res;
            }
        }
        // 0x1000 Battery Level Status
        fi = FeatureIndex(c, dev, 0x1000);
        if (fi > 0)
        {
            var s = Request(c, dev, (byte)fi, 0, 400, out err);
            if (s != null)
            {
                res.Battery = s[0] + "%" + (s[2] == 1 || s[2] == 2 ? " (charging)" : s[2] == 3 ? " (full)" : "");
                res.Source = "HID++ 0x1000 battery status";
                return res;
            }
        }
        // 0x1001 Battery Voltage
        fi = FeatureIndex(c, dev, 0x1001);
        if (fi > 0)
        {
            var s = Request(c, dev, (byte)fi, 0, 400, out err);
            if (s != null)
            {
                int mv = (s[0] << 8) | s[1];
                res.Battery = "~" + VoltageToPercent(mv) + "% (" + mv + " mV)" + ((s[2] & 0x80) != 0 ? " (charging)" : "");
                res.Source = "HID++ 0x1001 battery voltage";
                return res;
            }
        }
        // 0x1F20 ADC measurement (G series headsets)
        fi = FeatureIndex(c, dev, 0x1F20);
        if (fi > 0)
        {
            var s = Request(c, dev, (byte)fi, 0, 400, out err);
            if (s != null)
            {
                int mv = (s[0] << 8) | s[1];
                if (mv == 0) { res.Battery = "unknown"; res.Notes = "headset off or not reporting"; }
                else res.Battery = "~" + VoltageToPercent(mv) + "% (" + mv + " mV)" + (s[2] == 3 ? " (charging)" : "");
                res.Source = "HID++ 0x1F20 ADC measurement";
                return res;
            }
        }
        res.Notes = "no battery (wired device)";
        return res;
    }

    public static List<HidPPResult> Probe()
    {
        var results = new List<HidPPResult>();
        foreach (var path in HidPaths())
        {
            if (path.IndexOf("vid_046d", StringComparison.OrdinalIgnoreCase) < 0) continue;
            var h = CreateFile(path, 0xC0000000, 3, IntPtr.Zero, 3, 0x40000000, IntPtr.Zero);
            if (h.IsInvalid) continue;
            var caps = new HIDP_CAPS(); IntPtr pd;
            if (!HidD_GetPreparsedData(h, out pd)) { h.Dispose(); continue; }
            HidP_GetCaps(pd, ref caps); HidD_FreePreparsedData(pd);
            // HID++ long-report collection: vendor usage page, 20-byte reports.
            if (caps.UsagePage < 0xFF00 || caps.InputReportByteLength != 20 || caps.OutputReportByteLength != 20) { h.Dispose(); continue; }
            var sb = new StringBuilder(128); HidD_GetProductString(h, sb, 256);
            var attr = new HIDD_ATTRIBUTES(); attr.Size = Marshal.SizeOf(attr); HidD_GetAttributes(h, ref attr);
            string iface = string.Format("{0} (PID {1:X4})", sb, attr.ProductID);
            using (var c = new Channel(h, caps.InputReportByteLength, caps.OutputReportByteLength))
            {
                bool any = false;
                foreach (byte dev in new byte[] { 0xFF, 1, 2, 3, 4, 5, 6 })
                {
                    var r = ProbeDevice(c, dev);
                    if (r == null) continue;
                    if (dev == 0xFF && r.Battery == "" && r.Kind == "receiver") continue;
                    r.Interface = iface; r.Product = sb.ToString(); r.Path = path;
                    results.Add(r); any = true;
                    if (dev == 0xFF) break; // direct device (wired, or headset dongle that ignores the slot number)
                }
                if (!any) results.Add(new HidPPResult { Interface = iface, Path = path, Notes = "no HID++ 2.0 device answered (asleep, off, or HID++ 1.0)" });
            }
        }
        return results;
    }
}
