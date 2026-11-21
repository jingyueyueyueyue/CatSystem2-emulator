/*
 * B / R3: one bounded D3D8 oFog observation, not a simulator implementation.
 * Source evidence: archive/2026-11/20261117_F_used_shader_equations.json,
 * BillboardYAdd / INPUT_00. The cache contains assembly TEXT, not original
 * assembled bytecode. Tokens below encode those three instructions directly;
 * the controls append only mov oFog.x, c5.x (zero) / c6.x (one).
 * No D3DX, game files, game DLLs, reference implementation, or auto fallback.
 *
 * Build ONLY WHEN THE MAIN AGENT RUNS IT (PowerShell, workspace cwd):
 * & C:/msys64/mingw64/bin/gcc.exe -std=c11 -O2 -Wall -Wextra -municode scripts/analysis/format_B_d3d8_unwritten_fog_probe.c -o "$env:TEMP/20261118_format_B_d3d8_fog_probe.exe" -luser32
 * Usage: executable [output.json] (omitting the path writes JSON to stdout).
 * Put the observation in archive/2026-11/20261118_format_B_d3d8_fog.json.
 * The author has NOT compiled or executed this probe.
 */
#define WIN32_LEAN_AND_MEAN
#define COBJMACROS
#include <windows.h>
#include <d3d8.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>

enum { WIDTH = 64, HEIGHT = 64, SAMPLE_X = 24, SAMPLE_Y = 28 };
#define SOURCE_RGB 0x00c04020UL
#define FOG_RGB    0x001040c0UL
#define VP_FLAGS (D3DCREATE_HARDWARE_VERTEXPROCESSING | D3DCREATE_FPU_PRESERVE)

typedef struct { const char *api; HRESULT hr; DWORD arg0, arg1; } Call;
typedef struct { Call calls[96]; unsigned count, failures; BOOL overflow; } Log;
typedef struct {
    const char *name;
    DWORD code[16], pixel;
    unsigned words;
    BOOL pixel_valid;
    Log log;
} Draw;
typedef struct {
    unsigned minor;
    BOOL attempted, setup_ok;
    D3DSURFACE_DESC target_desc;
    Log log;
    Draw draws[3];
} Probe;
typedef IDirect3D8 *(WINAPI *CreateD3D8)(UINT);

typedef struct { float position[3], normal[3], uv[2]; } Vertex;
static const Vertex quad[4] = {
    {{-.75f, .75f, .5f}, {0, 0, 1}, {0, 0}},
    {{ .75f, .75f, .5f}, {0, 0, 1}, {1, 0}},
    {{-.75f,-.75f, .5f}, {0, 0, 1}, {0, 1}},
    {{ .75f,-.75f, .5f}, {0, 0, 1}, {1, 1}}
};
static const DWORD declaration[] = {
    D3DVSD_STREAM(0),
    D3DVSD_REG(D3DVSDE_POSITION, D3DVSDT_FLOAT3),
    D3DVSD_REG(D3DVSDE_NORMAL, D3DVSDT_FLOAT3),
    D3DVSD_REG(D3DVSDE_TEXCOORD0, D3DVSDT_FLOAT2),
    D3DVSD_END()
};
_Static_assert(sizeof(DWORD) == 4, "D3D8 tokens must be DWORDs");
_Static_assert(sizeof(Vertex) == 32, "FVF 0x112 input stride must be 32");

static BOOL record(Log *log, const char *api, HRESULT hr, DWORD a, DWORD b)
{
    if (log->count < sizeof(log->calls) / sizeof(log->calls[0]))
        log->calls[log->count++] = (Call){api, hr, a, b};
    else { log->overflow = TRUE; ++log->failures; return FALSE; }
    if (FAILED(hr)) ++log->failures;
    return SUCCEEDED(hr);
}

static DWORD float_bits(float value)
{
    DWORD bits;
    memcpy(&bits, &value, sizeof(bits));
    return bits;
}

static void make_shader(Draw *draw, unsigned minor, unsigned variant)
{
    /* m4x4 oPos, v0, c0; mov oD0, c4; add oT0, v7, c20 */
    static const DWORD body[] = {
        0x00000014, 0xc00f0000, 0x90e40000, 0xa0e40000,
        0x00000001, 0xd00f0000, 0xa0e40004,
        0x00000002, 0xe00f0000, 0x90e40007, 0xa0e40014
    };
    draw->name = variant == 0 ? "unwritten" : variant == 1 ? "explicit_0" : "explicit_1";
    draw->code[0] = D3DVS_VERSION(1, minor);
    memcpy(draw->code + 1, body, sizeof(body));
    draw->words = 1 + (unsigned)(sizeof(body) / sizeof(body[0]));
    if (variant) {
        draw->code[draw->words++] = D3DSIO_MOV;
        draw->code[draw->words++] = 0xc0010001; /* oFog.x */
        draw->code[draw->words++] = variant == 1 ? 0xa0000005 : 0xa0000006;
    }
    draw->code[draw->words++] = D3DSIO_END;
}

static BOOL set_rs(IDirect3DDevice8 *dev, Log *log, D3DRENDERSTATETYPE state, DWORD value)
{
    return record(log, "SetRenderState", IDirect3DDevice8_SetRenderState(dev, state, value),
                  (DWORD)state, value);
}

static BOOL set_ts(IDirect3DDevice8 *dev, Log *log, D3DTEXTURESTAGESTATETYPE state, DWORD value)
{
    return record(log, "SetTextureStageState[0]",
                  IDirect3DDevice8_SetTextureStageState(dev, 0, state, value), (DWORD)state, value);
}

static void draw_once(IDirect3DDevice8 *dev, IDirect3DSurface8 *target, Draw *draw)
{
    DWORD shader = 0;
    D3DLOCKED_RECT locked;
    BOOL ok;
    if (!record(&draw->log, "CreateVertexShader",
            IDirect3DDevice8_CreateVertexShader(dev, declaration, draw->code, &shader, 0), 0, 0))
        return;
    ok = record(&draw->log, "SetVertexShader",
                IDirect3DDevice8_SetVertexShader(dev, shader), shader, 0);
    if (ok) ok = record(&draw->log, "Clear",
                IDirect3DDevice8_Clear(dev, 0, NULL, D3DCLEAR_TARGET, 0xff000000, 1.0f, 0), 0, 0);
    if (ok) ok = record(&draw->log, "BeginScene", IDirect3DDevice8_BeginScene(dev), 0, 0);
    if (ok) {
        ok = record(&draw->log, "DrawPrimitiveUP",
                    IDirect3DDevice8_DrawPrimitiveUP(dev, D3DPT_TRIANGLESTRIP, 2, quad, sizeof(Vertex)), 2, sizeof(Vertex));
        /* End a successful BeginScene even if DrawPrimitiveUP failed. */
        ok = record(&draw->log, "EndScene", IDirect3DDevice8_EndScene(dev), 0, 0) && ok;
    }
    if (ok && record(&draw->log, "LockRect[target]",
            IDirect3DSurface8_LockRect(target, &locked, NULL, D3DLOCK_READONLY), D3DLOCK_READONLY, 0)) {
        memcpy(&draw->pixel, (const unsigned char *)locked.pBits
               + (ptrdiff_t)SAMPLE_Y * locked.Pitch + SAMPLE_X * 4, sizeof(draw->pixel));
        draw->pixel_valid = record(&draw->log, "UnlockRect[target]",
                                  IDirect3DSurface8_UnlockRect(target), 0, 0);
    }
    record(&draw->log, "SetVertexShader[unbind]",
           IDirect3DDevice8_SetVertexShader(dev, D3DFVF_XYZ), D3DFVF_XYZ, 0);
    record(&draw->log, "DeleteVertexShader",
           IDirect3DDevice8_DeleteVertexShader(dev, shader), shader, 0);
}

static void run_probe(IDirect3D8 *d3d, HWND window, D3DFORMAT display_format, Probe *probe)
{
    IDirect3DDevice8 *dev = NULL;
    IDirect3DSurface8 *target = NULL;
    IDirect3DTexture8 *white = NULL;
    D3DLOCKED_RECT locked;
    D3DPRESENT_PARAMETERS pp;
    D3DVIEWPORT8 viewport = {0, 0, WIDTH, HEIGHT, 0.0f, 1.0f};
    float constants[21][4] = {{0}};
    BOOL ok = TRUE;
    unsigned i;
    probe->attempted = TRUE;
    memset(&pp, 0, sizeof(pp));
    pp.Windowed = TRUE;
    pp.hDeviceWindow = window;
    pp.BackBufferWidth = WIDTH;
    pp.BackBufferHeight = HEIGHT;
    pp.BackBufferFormat = display_format;
    pp.BackBufferCount = 1;
    pp.SwapEffect = D3DSWAPEFFECT_DISCARD;
    if (!record(&probe->log, "CreateDevice[HAL,HARDWARE_VP]",
            IDirect3D8_CreateDevice(d3d, D3DADAPTER_DEFAULT, D3DDEVTYPE_HAL, window,
                                   VP_FLAGS, &pp, &dev), VP_FLAGS, display_format)) goto done;
    if (!record(&probe->log, "CreateRenderTarget[A8R8G8B8,lockable]",
            IDirect3DDevice8_CreateRenderTarget(dev, WIDTH, HEIGHT, D3DFMT_A8R8G8B8,
                D3DMULTISAMPLE_NONE, TRUE, &target), WIDTH, HEIGHT)) goto done;
    if (!record(&probe->log, "GetDesc[target]",
            IDirect3DSurface8_GetDesc(target, &probe->target_desc), 0, 0)) goto done;
    if (probe->target_desc.Format != D3DFMT_A8R8G8B8) goto done;
    if (!record(&probe->log, "SetRenderTarget",
            IDirect3DDevice8_SetRenderTarget(dev, target, NULL), 0, 0)) goto done;
    ok &= record(&probe->log, "SetViewport", IDirect3DDevice8_SetViewport(dev, &viewport), WIDTH, HEIGHT);
    if (!record(&probe->log, "CreateTexture[white]",
            IDirect3DDevice8_CreateTexture(dev, 1, 1, 1, 0, D3DFMT_A8R8G8B8,
                                           D3DPOOL_MANAGED, &white), 1, 1)) goto done;
    if (!record(&probe->log, "LockRect[white]",
            IDirect3DTexture8_LockRect(white, 0, &locked, NULL, 0), 0, 0)) goto done;
    { DWORD pixel = 0xffffffff; memcpy(locked.pBits, &pixel, sizeof(pixel)); }
    ok &= record(&probe->log, "UnlockRect[white]", IDirect3DTexture8_UnlockRect(white, 0), 0, 0);
    ok &= record(&probe->log, "SetTexture[0]",
                 IDirect3DDevice8_SetTexture(dev, 0, (IDirect3DBaseTexture8 *)white), 0, 0);
    ok &= record(&probe->log, "SetPixelShader[fixed_function]",
                 IDirect3DDevice8_SetPixelShader(dev, 0), 0, 0);
    /* Match the additive/white-texture color path, isolating fog from depth
       and culling. There is no Present or front-buffer/desktop capture. */
    ok &= set_rs(dev, &probe->log, D3DRS_ZENABLE, D3DZB_FALSE);
    ok &= set_rs(dev, &probe->log, D3DRS_ZWRITEENABLE, FALSE);
    ok &= set_rs(dev, &probe->log, D3DRS_CULLMODE, D3DCULL_NONE);
    ok &= set_rs(dev, &probe->log, D3DRS_LIGHTING, FALSE);
    ok &= set_rs(dev, &probe->log, D3DRS_SPECULARENABLE, FALSE);
    ok &= set_rs(dev, &probe->log, D3DRS_DITHERENABLE, FALSE);
    ok &= set_rs(dev, &probe->log, D3DRS_SHADEMODE, D3DSHADE_GOURAUD);
    ok &= set_rs(dev, &probe->log, D3DRS_ALPHABLENDENABLE, TRUE);
    ok &= set_rs(dev, &probe->log, D3DRS_SRCBLEND, D3DBLEND_SRCALPHA);
    ok &= set_rs(dev, &probe->log, D3DRS_DESTBLEND, D3DBLEND_ONE);
    ok &= set_rs(dev, &probe->log, D3DRS_ALPHATESTENABLE, TRUE);
    ok &= set_rs(dev, &probe->log, D3DRS_ALPHAFUNC, D3DCMP_GREATEREQUAL);
    ok &= set_rs(dev, &probe->log, D3DRS_ALPHAREF, 1);
    ok &= set_rs(dev, &probe->log, D3DRS_FOGENABLE, TRUE);
    ok &= set_rs(dev, &probe->log, D3DRS_FOGTABLEMODE, D3DFOG_NONE);
    ok &= set_rs(dev, &probe->log, D3DRS_FOGVERTEXMODE, D3DFOG_LINEAR);
    ok &= set_rs(dev, &probe->log, D3DRS_RANGEFOGENABLE, FALSE);
    ok &= set_rs(dev, &probe->log, D3DRS_FOGCOLOR, FOG_RGB);
    ok &= set_rs(dev, &probe->log, D3DRS_FOGSTART, float_bits(0.0f));
    ok &= set_rs(dev, &probe->log, D3DRS_FOGEND, float_bits(1.0f));
    ok &= set_rs(dev, &probe->log, D3DRS_FOGDENSITY, float_bits(1.0f));
    ok &= set_ts(dev, &probe->log, D3DTSS_COLOROP, D3DTOP_MODULATE);
    ok &= set_ts(dev, &probe->log, D3DTSS_COLORARG1, D3DTA_TEXTURE);
    ok &= set_ts(dev, &probe->log, D3DTSS_COLORARG2, D3DTA_DIFFUSE);
    ok &= set_ts(dev, &probe->log, D3DTSS_ALPHAOP, D3DTOP_MODULATE);
    ok &= set_ts(dev, &probe->log, D3DTSS_ALPHAARG1, D3DTA_TEXTURE);
    ok &= set_ts(dev, &probe->log, D3DTSS_ALPHAARG2, D3DTA_DIFFUSE);
    ok &= set_ts(dev, &probe->log, D3DTSS_MINFILTER, D3DTEXF_POINT);
    ok &= set_ts(dev, &probe->log, D3DTSS_MAGFILTER, D3DTEXF_POINT);
    ok &= set_ts(dev, &probe->log, D3DTSS_MIPFILTER, D3DTEXF_NONE);
    ok &= record(&probe->log, "SetTextureStageState[1,COLOROP]",
            IDirect3DDevice8_SetTextureStageState(dev, 1, D3DTSS_COLOROP, D3DTOP_DISABLE), D3DTSS_COLOROP, D3DTOP_DISABLE);
    for (i = 0; i < 4; ++i) constants[i][i] = constants[10 + i][i] = 1.0f;
    constants[4][0] = 192.0f / 255.0f;
    constants[4][1] = 64.0f / 255.0f;
    constants[4][2] = 32.0f / 255.0f;
    constants[4][3] = 1.0f;
    for (i = 0; i < 4; ++i) constants[6][i] = 1.0f; /* c5 remains zero */
    constants[14][0] = 1.0f; /* supplied but not read by the cached shader */
    ok &= record(&probe->log, "SetVertexShaderConstant[c0..c20]",
                 IDirect3DDevice8_SetVertexShaderConstant(dev, 0, constants, 21), 0, 21);
    probe->setup_ok = ok;
    if (ok) for (i = 0; i < 3; ++i) draw_once(dev, target, &probe->draws[i]);
done:
    if (white) IDirect3DTexture8_Release(white);
    if (target) IDirect3DSurface8_Release(target);
    if (dev) IDirect3DDevice8_Release(dev);
}

static void json_wide(FILE *out, const wchar_t *text)
{
    fputc('"', out);
    while (*text) {
        unsigned c = (unsigned short)*text++;
        if (c >= 32 && c < 127 && c != '"' && c != '\\') fputc((int)c, out);
        else fprintf(out, "\\u%04x", c);
    }
    fputc('"', out);
}

static void json_ansi(FILE *out, const char *text)
{
    wchar_t wide[MAX_DEVICE_IDENTIFIER_STRING + 1];
    if (MultiByteToWideChar(CP_ACP, 0, text, -1, wide,
                           (int)(sizeof(wide) / sizeof(wide[0])))) json_wide(out, wide);
    else fputs("null", out);
}

static void json_hex(FILE *out, const void *bytes, size_t length)
{
    const unsigned char *p = (const unsigned char *)bytes;
    size_t i;
    fputc('"', out);
    for (i = 0; i < length; ++i) fprintf(out, "%02x", (unsigned)p[i]);
    fputc('"', out);
}

static void json_log(FILE *out, const Log *log)
{
    unsigned i;
    fputs("[", out);
    for (i = 0; i < log->count; ++i) {
        const Call *c = &log->calls[i];
        fprintf(out, "%s{\"api\":\"%s\",\"hr\":\"0x%08lx\",\"ok\":%s,"
                "\"arg0\":\"0x%08lx\",\"arg1\":\"0x%08lx\"}", i ? "," : "",
                c->api, (unsigned long)(DWORD)c->hr, SUCCEEDED(c->hr) ? "true" : "false",
                (unsigned long)c->arg0, (unsigned long)c->arg1);
    }
    fputs("]", out);
}

static BOOL rgb_near(DWORD a, DWORD b)
{
    unsigned shift;
    for (shift = 0; shift < 24; shift += 8) {
        int delta = (int)((a >> shift) & 255) - (int)((b >> shift) & 255);
        if (delta < -1 || delta > 1) return FALSE;
    }
    return TRUE;
}

static BOOL controls_valid(const Probe *probe)
{
    return probe->setup_ok && probe->draws[1].pixel_valid && probe->draws[2].pixel_valid
        && !probe->draws[1].log.failures && !probe->draws[2].log.failures
        && rgb_near(probe->draws[1].pixel, FOG_RGB)
        && rgb_near(probe->draws[2].pixel, SOURCE_RGB);
}

static BOOL json_probe(FILE *out, const Probe *probe)
{
    unsigned i;
    BOOL controls = controls_valid(probe), complete = controls;
    fprintf(out, "{\"version\":\"vs.1.%u\",\"attempted\":%s,\"setup_ok\":%s,"
            "\"fresh_device_for_version\":true,\"setup_calls\":", probe->minor,
            probe->attempted ? "true" : "false", probe->setup_ok ? "true" : "false");
    json_log(out, &probe->log);
    fprintf(out, ",\"target_format\":%u,\"controls_valid_rgb_tolerance_1\":%s,\"draws\":[",
            (unsigned)probe->target_desc.Format, controls ? "true" : "false");
    for (i = 0; i < 3; ++i) {
        const Draw *draw = &probe->draws[i];
        BOOL valid = draw->pixel_valid && !draw->log.failures && !draw->log.overflow;
        complete &= valid;
        fprintf(out, "%s{\"name\":\"%s\",\"shader_bytes_le\":", i ? "," : "", draw->name);
        json_hex(out, draw->code, draw->words * sizeof(DWORD));
        fprintf(out, ",\"shader_dwords\":%u,\"calls\":", draw->words);
        json_log(out, &draw->log);
        fprintf(out, ",\"pixel_valid\":%s,\"argb\":", valid ? "true" : "false");
        if (valid) fprintf(out, "\"0x%08lx\",\"rgba\":[%u,%u,%u,%u]", (unsigned long)draw->pixel,
                (unsigned)((draw->pixel >> 16) & 255), (unsigned)((draw->pixel >> 8) & 255),
                (unsigned)(draw->pixel & 255), (unsigned)(draw->pixel >> 24));
        else fputs("null,\"rgba\":null", out);
        if (i == 0) {
            fputs(",\"matches_explicit_0_rgb\":", out);
            fputs(!controls || !valid ? "null" : rgb_near(draw->pixel, probe->draws[1].pixel) ? "true" : "false", out);
            fputs(",\"matches_explicit_1_rgb\":", out);
            fputs(!controls || !valid ? "null" : rgb_near(draw->pixel, probe->draws[2].pixel) ? "true" : "false", out);
        }
        fputc('}', out);
    }
    fprintf(out, "],\"complete\":%s}", complete ? "true" : "false");
    return complete;
}

int wmain(int argc, wchar_t **argv)
{
    FILE *out = stdout;
    HMODULE module = NULL;
    IDirect3D8 *d3d = NULL;
    HWND window = NULL;
    WNDCLASSW wc;
    wchar_t path[MAX_PATH + 32] = {0};
    D3DADAPTER_IDENTIFIER8 adapter;
    D3DCAPS8 caps;
    D3DDISPLAYMODE mode;
    Probe probes[2];
    Log log = {0};
    CreateD3D8 create;
    BOOL have_adapter = FALSE, have_caps = FALSE, ok = TRUE;
    BOOL class_registered = FALSE;
    unsigned i, j, length;
    if (argc > 2) { fputs("usage: probe.exe [output.json]\n", stderr); return 2; }
    if (argc == 2 && !(out = _wfopen(argv[1], L"wb"))) {
        fputs("cannot open JSON output\n", stderr); return 2;
    }
    memset(&wc, 0, sizeof(wc));
    memset(&adapter, 0, sizeof(adapter));
    memset(&caps, 0, sizeof(caps));
    memset(&mode, 0, sizeof(mode));
    memset(probes, 0, sizeof(probes));
    for (i = 0; i < 2; ++i) {
        probes[i].minor = i;
        for (j = 0; j < 3; ++j) make_shader(&probes[i].draws[j], i, j);
    }
    /* Absolute system path avoids loading a game-local d3d8 wrapper. */
    length = GetSystemDirectoryW(path, MAX_PATH);
    if (!record(&log, "GetSystemDirectoryW", length && length < MAX_PATH ? S_OK : E_FAIL,
                length && length < MAX_PATH ? 0 : GetLastError(), length)) goto output;
    wcscat(path, L"\\d3d8.dll");
    module = LoadLibraryW(path);
    if (!record(&log, "LoadLibraryW[system_d3d8]", module ? S_OK : E_FAIL,
                module ? 0 : GetLastError(), 0)) goto output;
    /* memcpy avoids a C object/function-pointer cast; both Win32 ABI pointer
       types returned by GetProcAddress have the same representation. */
    { FARPROC proc = GetProcAddress(module, "Direct3DCreate8");
      _Static_assert(sizeof(proc) == sizeof(create), "Win32 function pointer size");
      memcpy(&create, &proc, sizeof(create)); }
    if (!record(&log, "GetProcAddress[Direct3DCreate8]", create ? S_OK : E_FAIL,
                create ? 0 : GetLastError(), 0)) goto output;
    d3d = create(D3D_SDK_VERSION);
    if (!record(&log, "Direct3DCreate8", d3d ? S_OK : E_FAIL, D3D_SDK_VERSION, 0)) goto output;
    have_adapter = record(&log, "GetAdapterIdentifier[default]",
        IDirect3D8_GetAdapterIdentifier(d3d, D3DADAPTER_DEFAULT, 0, &adapter), 0, 0);
    have_caps = record(&log, "GetDeviceCaps[default,HAL]",
        IDirect3D8_GetDeviceCaps(d3d, D3DADAPTER_DEFAULT, D3DDEVTYPE_HAL, &caps), 0, D3DDEVTYPE_HAL);
    if (!record(&log, "GetAdapterDisplayMode[default]",
            IDirect3D8_GetAdapterDisplayMode(d3d, D3DADAPTER_DEFAULT, &mode), 0, 0)) goto output;
    wc.lpfnWndProc = DefWindowProcW;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"CS2_B_D3D8_FogProbe";
    class_registered = RegisterClassW(&wc) != 0;
    if (!record(&log, "RegisterClassW", class_registered ? S_OK : E_FAIL,
                class_registered ? 0 : GetLastError(), 0)) goto output;
    window = CreateWindowExW(0, wc.lpszClassName, L"D3D8 oFog reference probe", WS_POPUP,
                            0, 0, WIDTH, HEIGHT, NULL, NULL, wc.hInstance, NULL);
    if (!record(&log, "CreateWindowExW[hidden]", window ? S_OK : E_FAIL,
                window ? 0 : GetLastError(), 0)) goto output;
    /* Exactly six draws, no retry/order permutation and no other adapter,
       device type, software VP, D3D9, or format fallback. */
    for (i = 0; i < 2; ++i) run_probe(d3d, window, mode.Format, &probes[i]);
output:
    fputs("{\"schema\":\"cs2-r3-d3d8-ofog-probe/v1\",\"system_d3d8\":", out);
    json_wide(out, path);
    fprintf(out, ",\"process_bits\":%u,\"ansi_code_page\":%u,\"global_calls\":",
            (unsigned)(sizeof(void *) * 8), GetACP());
    json_log(out, &log);
    fputs(",\"adapter\":", out);
    if (have_adapter) {
        fputs("{\"driver\":", out); json_ansi(out, adapter.Driver);
        fputs(",\"description\":", out); json_ansi(out, adapter.Description);
        fprintf(out, ",\"driver_version\":\"0x%08lx%08lx\",\"vendor_id\":%lu,\"device_id\":%lu,"
                "\"subsys_id\":%lu,\"revision\":%lu,\"whql\":%lu,\"device_guid_raw\":",
                (unsigned long)(DWORD)adapter.DriverVersion.HighPart, (unsigned long)adapter.DriverVersion.LowPart,
                (unsigned long)adapter.VendorId, (unsigned long)adapter.DeviceId, (unsigned long)adapter.SubSysId,
                (unsigned long)adapter.Revision, (unsigned long)adapter.WHQLLevel);
        json_hex(out, &adapter.DeviceIdentifier, sizeof(adapter.DeviceIdentifier)); fputc('}', out);
    } else fputs("null", out);
    fputs(",\"caps\":", out);
    if (have_caps) {
        fprintf(out, "{\"vertex_shader_version\":\"0x%08lx\",\"max_vertex_shader_const\":%lu,"
                "\"raster_caps\":\"0x%08lx\",\"vertex_processing_caps\":\"0x%08lx\","
                "\"dev_caps\":\"0x%08lx\",\"fog_vertex\":%s,\"fog_table\":%s,\"raw_struct_le\":",
                (unsigned long)caps.VertexShaderVersion, (unsigned long)caps.MaxVertexShaderConst,
                (unsigned long)caps.RasterCaps, (unsigned long)caps.VertexProcessingCaps,
                (unsigned long)caps.DevCaps, caps.RasterCaps & D3DPRASTERCAPS_FOGVERTEX ? "true" : "false",
                caps.RasterCaps & D3DPRASTERCAPS_FOGTABLE ? "true" : "false");
        json_hex(out, &caps, sizeof(caps)); fputc('}', out);
    } else fputs("null", out);
    fputs(",\"source_evidence\":\"20261117_F_used_shader_equations.json:BillboardYAdd/INPUT_00\","
          "\"shader_provenance\":\"encoded_from_cached_assembly_text_not_original_assembler_bytes\","
          "\"assembly_body\":[\"m4x4 oPos, v0, c0\",\"mov oD0, c4\",\"add oT0, v7, c20\"],"
          "\"control_suffixes\":[null,\"mov oFog.x, c5.x\",\"mov oFog.x, c6.x\"],"
          "\"constant_layout\":\"c0..3 identity; c4 source rgba; c5 zero; c6 one; c10..13 identity; c14=(1,0,0,0); others through c20 zero\","
          "\"declaration_bytes_le\":", out);
    json_hex(out, declaration, sizeof(declaration));
    fprintf(out, ",\"vertex_stride\":%u,\"vertices_bytes_le\":", (unsigned)sizeof(Vertex));
    json_hex(out, quad, sizeof(quad));
    fprintf(out, ",\"sample_xy\":[%u,%u],\"dimensions\":[%u,%u],\"source_rgb\":\"0x%06lx\","
            "\"fog_rgb\":\"0x%06lx\",\"clear_rgb\":\"0x000000\",\"versions\":[",
            SAMPLE_X, SAMPLE_Y, WIDTH, HEIGHT, SOURCE_RGB, FOG_RGB);
    ok = !log.failures && !log.overflow && have_adapter && have_caps;
    for (i = 0; i < 2; ++i) { if (i) fputc(',', out); ok &= json_probe(out, &probes[i]); }
    fprintf(out, "],\"complete\":%s,\"scope\":\"host observation only; undefined output may depend on driver and prior state; no game GPU value or portable default is inferred\"}\n",
            ok ? "true" : "false");
    if (out == stdout) { if (fflush(out) || ferror(out)) ok = FALSE; }
    else if (fclose(out)) ok = FALSE;
    if (d3d) IDirect3D8_Release(d3d);
    if (window) DestroyWindow(window);
    if (class_registered) UnregisterClassW(wc.lpszClassName, wc.hInstance);
    if (module) FreeLibrary(module);
    return ok ? 0 : 1;
}
