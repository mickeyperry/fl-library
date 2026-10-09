# Dumps what the property system returns for a file: proptest.ps1 <path> [-Direct]
param([string]$Path, [switch]$Direct)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class PropTest {
    [StructLayout(LayoutKind.Sequential, Pack = 4)] public struct PROPERTYKEY { public Guid fmtid; public uint pid; }
    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IPropertyStore {
        [PreserveSig] int GetCount(out uint c);
        [PreserveSig] int GetAt(uint i, out PROPERTYKEY k);
        [PreserveSig] int GetValue(ref PROPERTYKEY k, IntPtr pv);
        [PreserveSig] int SetValue(ref PROPERTYKEY k, IntPtr pv);
        [PreserveSig] int Commit();
    }
    [ComImport, Guid("B7D14566-0509-4CCE-A71F-0A554233BD9B"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IInitializeWithFile { [PreserveSig] int Initialize([MarshalAs(UnmanagedType.LPWStr)] string path, uint mode); }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)] static extern int SHGetPropertyStoreFromParsingName(string path, IntPtr bc, int flags, ref Guid riid, out IPropertyStore store);
    [DllImport("propsys.dll", CharSet = CharSet.Unicode)] static extern int PSGetNameFromPropertyKey(ref PROPERTYKEY k, out IntPtr name);
    [DllImport("propsys.dll", CharSet = CharSet.Unicode)] static extern int PropVariantToString(IntPtr pv, StringBuilder sb, int cch);
    [DllImport("propsys.dll", CharSet = CharSet.Unicode)] static extern int PSFormatForDisplay(ref PROPERTYKEY k, IntPtr pv, int flags, StringBuilder sb, int cch);
    [DllImport("ole32.dll")] static extern int PropVariantClear(IntPtr pv);
    public static string Dump(string path, bool direct) {
        var sb = new StringBuilder();
        IPropertyStore store;
        if (direct) {
            object o = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("7C0B4E7A-52F1-4B8E-9D7C-2F1A6B0C4D11")));
            int hi = ((IInitializeWithFile)o).Initialize(path, 0);
            sb.AppendLine("Initialize hr=0x" + hi.ToString("X8"));
            store = (IPropertyStore)o;
        } else {
            Guid iid = new Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99");
            int hr = SHGetPropertyStoreFromParsingName(path, IntPtr.Zero, 0, ref iid, out store);
            sb.AppendLine("SHGetPropertyStoreFromParsingName hr=0x" + hr.ToString("X8"));
            if (hr < 0) return sb.ToString();
        }
        uint n; store.GetCount(out n);
        sb.AppendLine("count=" + n);
        IntPtr pv = Marshal.AllocCoTaskMem(32);
        for (uint i = 0; i < n; i++) {
            PROPERTYKEY k; store.GetAt(i, out k);
            for (int b = 0; b < 32; b++) Marshal.WriteByte(pv, b, 0);
            store.GetValue(ref k, pv);
            IntPtr pn; string name = k.fmtid + " " + k.pid;
            if (PSGetNameFromPropertyKey(ref k, out pn) >= 0) { name = Marshal.PtrToStringUni(pn); Marshal.FreeCoTaskMem(pn); }
            var v = new StringBuilder(1024); var d = new StringBuilder(1024);
            PropVariantToString(pv, v, 1024); PSFormatForDisplay(ref k, pv, 0, d, 1024);
            if (name.StartsWith("FLLibrary") || name.Contains("Music") || name == "System.Title" || name == "System.Rating" || name == "System.Keywords" || name == "System.Author" || name == "System.Comment" || direct)
                sb.AppendLine(String.Format("{0,-30} vt={1,-5} {2}  [{3}]", name, Marshal.ReadInt16(pv), v, d));
            PropVariantClear(pv);
        }
        return sb.ToString();
    }
}
'@
[PropTest]::Dump($Path, [bool]$Direct)
