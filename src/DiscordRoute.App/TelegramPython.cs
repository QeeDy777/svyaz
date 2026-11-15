using System.Runtime.InteropServices;

namespace DiscordRoute.App;

// One interpreter per process: CPython and its native extensions cannot safely be unloaded/reinitialized.
internal static class TelegramPython
{
    private static readonly object Gate = new();
    private static nint library;
    private static nint module;
    private static Exception? initializationFailure;

    internal static void Initialize(string root)
    {
        lock (Gate)
        {
            if (module != 0) return;
            if (initializationFailure != null) throw new IOException("Перезапустите «Связь», чтобы повторить запуск движка Telegram.", initializationFailure);
            try
            {
                library = NativeLibrary.Load(Path.Combine(root, "python314.dll"), typeof(TelegramPython).Assembly,
                    DllImportSearchPath.UseDllDirectoryForDependencies | DllImportSearchPath.System32);
                NativeLibrary.SetDllImportResolver(typeof(TelegramPython).Assembly,
                    (name, _, _) => name == "svyaz-python" ? library : 0);
                var config = PyInitConfig_Create();
                if (config == 0) throw new OutOfMemoryException();
                try
                {
                    void Check(int code)
                    {
                        if (code >= 0) return;
                        PyInitConfig_GetError(config, out var error);
                        throw new IOException("Не удалось загрузить Python: " + Marshal.PtrToStringUTF8(error));
                    }
                    Check(PyInitConfig_SetInt(config, "site_import", 0));
                    Check(PyInitConfig_SetInt(config, "write_bytecode", 0));
                    Check(PyInitConfig_SetInt(config, "install_signal_handlers", 0));
                    Check(PyInitConfig_SetStr(config, "home", root));
                    var paths = new nint[2];
                    try
                    {
                        paths[0] = Marshal.StringToCoTaskMemUTF8(Path.Combine(root, "python314.zip"));
                        paths[1] = Marshal.StringToCoTaskMemUTF8(root);
                        Check(PyInitConfig_SetStrList(config, "module_search_paths", 2, paths));
                    }
                    finally { foreach (var path in paths) Marshal.FreeCoTaskMem(path); }
                    Check(Py_InitializeFromInitConfig(config));
                }
                finally { PyInitConfig_Free(config); }
                try
                {
                    module = PyImport_ImportModule("svyaz_proxy");
                    if (module == 0) throw PythonError("загрузка движка");
                }
                finally { PyEval_SaveThread(); }
            }
            catch (Exception ex) { initializationFailure = ex; throw; }
        }
    }

    internal static string? Call(string method, string? json = null)
    {
        var gil = PyGILState_Ensure();
        nint callable = 0, argument = 0, result = 0;
        try
        {
            callable = PyObject_GetAttrString(module, method);
            if (callable == 0) throw PythonError(method);
            if (json != null)
            {
                argument = PyUnicode_FromString(json);
                if (argument == 0) throw PythonError(method);
                result = PyObject_CallOneArg(callable, argument);
            }
            else result = PyObject_CallNoArgs(callable);
            if (result == 0) throw PythonError(method);
            return method == "status" ? Marshal.PtrToStringUTF8(PyUnicode_AsUTF8(result)) : null;
        }
        finally
        {
            if (result != 0) Py_DecRef(result);
            if (argument != 0) Py_DecRef(argument);
            if (callable != 0) Py_DecRef(callable);
            PyGILState_Release(gil);
        }
    }

    private static IOException PythonError(string operation)
    {
        var type = PyErr_Occurred();
        var name = type == 0 ? 0 : PyObject_GetAttrString(type, "__name__");
        var text = name == 0 ? "PythonError" : Marshal.PtrToStringUTF8(PyUnicode_AsUTF8(name));
        if (name != 0) Py_DecRef(name);
        PyErr_Clear();
        return new IOException($"Встроенный Telegram: {operation} — {text}.");
    }

    private const string Dll = "svyaz-python";
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern nint PyInitConfig_Create();
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern void PyInitConfig_Free(nint config);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern int PyInitConfig_GetError(nint config, out nint error);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern int PyInitConfig_SetInt(nint config, [MarshalAs(UnmanagedType.LPUTF8Str)] string name, long value);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern int PyInitConfig_SetStr(nint config, [MarshalAs(UnmanagedType.LPUTF8Str)] string name, [MarshalAs(UnmanagedType.LPUTF8Str)] string value);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern int PyInitConfig_SetStrList(nint config, [MarshalAs(UnmanagedType.LPUTF8Str)] string name, nuint length, [In] nint[] values);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern int Py_InitializeFromInitConfig(nint config);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern nint PyEval_SaveThread();
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern int PyGILState_Ensure();
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern void PyGILState_Release(int state);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern nint PyImport_ImportModule([MarshalAs(UnmanagedType.LPUTF8Str)] string name);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern nint PyObject_GetAttrString(nint obj, [MarshalAs(UnmanagedType.LPUTF8Str)] string name);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern nint PyUnicode_FromString([MarshalAs(UnmanagedType.LPUTF8Str)] string value);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern nint PyUnicode_AsUTF8(nint value);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern nint PyObject_CallOneArg(nint callable, nint argument);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern nint PyObject_CallNoArgs(nint callable);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern void Py_DecRef(nint value);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern nint PyErr_Occurred();
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)] private static extern void PyErr_Clear();
}
