#include <windows.h>
#include <d3d11.h>
#include <dxgi.h>
#include <wrl/client.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <limits>
#include <stdexcept>
#include <string>
#include <vector>

using Microsoft::WRL::ComPtr;

namespace {

constexpr UINT SOURCE_WIDTH = 240;
constexpr UINT SOURCE_HEIGHT = 108;
constexpr UINT TILE_SIZE = 16;
constexpr UINT PARTIAL_WIDTH = (SOURCE_WIDTH + TILE_SIZE - 1) / TILE_SIZE;
constexpr UINT PARTIAL_HEIGHT = (SOURCE_HEIGHT + TILE_SIZE - 1) / TILE_SIZE;

constexpr int CLIENT_WIDTH = 1280;
constexpr int CLIENT_HEIGHT = 410;
constexpr int PANEL_WIDTH = 360;
constexpr int PANEL_HEIGHT = 162;
constexpr int PANEL_Y = 150;
constexpr int PANEL_X[3] = { 40, 460, 880 };

HWND gWindow = nullptr;
HFONT gLabelFont = nullptr;
HBRUSH gLabelBrush = nullptr;

struct Float4 { float r, g, b, a; };

void Check(HRESULT hr, const char* operation) {
    if (FAILED(hr)) {
        char message[256]{};
        std::snprintf(message, sizeof(message), "%s failed (HRESULT 0x%08lx)",
                      operation, static_cast<unsigned long>(hr));
        throw std::runtime_error(message);
    }
}

std::vector<std::uint8_t> ReadBinary(const std::filesystem::path& path) {
    std::ifstream file(path, std::ios::binary);
    if (!file) throw std::runtime_error("Cannot open shader bytecode: " + path.string());
    return std::vector<std::uint8_t>(
        std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>());
}

std::uint32_t PackUFloat(float value, unsigned mantissaBits) {
    std::uint32_t bits = 0;
    std::memcpy(&bits, &value, sizeof(bits));
    const std::uint32_t exponent = (bits >> 23) & 0xffu;
    const std::uint32_t mantissa = bits & 0x7fffffu;

    if (exponent == 0xffu) {
        std::uint32_t encoded = 31u << mantissaBits;
        if (mantissa != 0) {
            std::uint32_t m = mantissa >> (23u - mantissaBits);
            if (m == 0) m = 1;
            encoded |= m;
        }
        return encoded;
    }

    const int e = static_cast<int>(exponent) - 127 + 15;
    if (e <= 0) return 0;
    if (e >= 31) return 31u << mantissaBits;

    return (static_cast<std::uint32_t>(e) << mantissaBits) |
           (mantissa >> (23u - mantissaBits));
}

float UnpackUFloat(std::uint32_t value, unsigned mantissaBits) {
    const std::uint32_t mantissaMask = (1u << mantissaBits) - 1u;
    const std::uint32_t exponent = (value >> mantissaBits) & 31u;
    const std::uint32_t mantissa = value & mantissaMask;

    if (exponent == 0) {
        return std::ldexp(static_cast<float>(mantissa),
                          -14 - static_cast<int>(mantissaBits));
    }
    if (exponent == 31) {
        return mantissa ? std::numeric_limits<float>::quiet_NaN()
                        : std::numeric_limits<float>::infinity();
    }

    const float significand =
        1.0f + static_cast<float>(mantissa) /
        static_cast<float>(1u << mantissaBits);
    return std::ldexp(significand, static_cast<int>(exponent) - 15);
}

std::uint32_t PackR11G11B10(const Float4& p) {
    return PackUFloat(p.r, 6) |
           (PackUFloat(p.g, 6) << 11) |
           (PackUFloat(p.b, 5) << 22);
}

Float4 UnpackR11G11B10(std::uint32_t packed) {
    return {
        UnpackUFloat(packed & 0x7ffu, 6),
        UnpackUFloat((packed >> 11) & 0x7ffu, 6),
        UnpackUFloat((packed >> 22) & 0x3ffu, 5),
        1.0f
    };
}

float CpuAverage(const std::vector<Float4>& pixels) {
    constexpr float wr = 0.21250000596046447754f;
    constexpr float wg = 0.71539998054504394531f;
    constexpr float wb = 0.0720999985933303833f;

    double sum = 0.0;
    for (const auto& p : pixels) {
        const float luminance = p.r * wr + p.g * wg + p.b * wb;
        if (std::isfinite(luminance)) sum += luminance;
    }
    return static_cast<float>(
        sum / (static_cast<double>(SOURCE_WIDTH) * SOURCE_HEIGHT));
}

LRESULT CALLBACK WindowProc(HWND hwnd, UINT message, WPARAM wParam, LPARAM lParam) {
    switch (message) {
    case WM_CTLCOLORSTATIC: {
        HDC dc = reinterpret_cast<HDC>(wParam);
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, RGB(214, 220, 232));
        return reinterpret_cast<LRESULT>(gLabelBrush);
    }
    case WM_ERASEBKGND:
        return 1;
    case WM_CLOSE:
        DestroyWindow(hwnd);
        return 0;
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    case WM_PAINT: {
        PAINTSTRUCT paint{};
        BeginPaint(hwnd, &paint);
        EndPaint(hwnd, &paint);
        return 0;
    }
    default:
        return DefWindowProc(hwnd, message, wParam, lParam);
    }
}

HWND AddLabel(HWND parent, const char* text, int x, int y, int width, int height) {
    HWND label = CreateWindowExA(
        0, "STATIC", text,
        WS_CHILD | WS_VISIBLE | SS_CENTER | SS_CENTERIMAGE,
        x, y, width, height, parent, nullptr,
        GetModuleHandle(nullptr), nullptr);
    if (label && gLabelFont) {
        SendMessage(label, WM_SETFONT, reinterpret_cast<WPARAM>(gLabelFont), TRUE);
    }
    return label;
}

struct Preview {
    ComPtr<ID3D11Device> device;
    ComPtr<ID3D11DeviceContext> context;
    ComPtr<IDXGISwapChain> swapChain;
    ComPtr<ID3D11RenderTargetView> backBufferRtv;

    ComPtr<ID3D11Texture2D> sourceTexture;
    ComPtr<ID3D11ShaderResourceView> sourceSrv;
    ComPtr<ID3D11Texture2D> partialTexture;
    ComPtr<ID3D11RenderTargetView> partialRtv;
    ComPtr<ID3D11ShaderResourceView> partialSrv;
    ComPtr<ID3D11Texture2D> finalTexture;
    ComPtr<ID3D11RenderTargetView> finalRtv;
    ComPtr<ID3D11ShaderResourceView> finalSrv;

    ComPtr<ID3D11VertexShader> reductionVs;
    ComPtr<ID3D11PixelShader> tilePs;
    ComPtr<ID3D11PixelShader> finalPs;
    ComPtr<ID3D11VertexShader> displayVs;
    ComPtr<ID3D11PixelShader> displayColorPs;
    ComPtr<ID3D11PixelShader> displayScalarPs;
    ComPtr<ID3D11Buffer> luminanceConstants;
    ComPtr<ID3D11Buffer> displayConstants;
    ComPtr<ID3D11SamplerState> pointSampler;
    ComPtr<ID3D11RasterizerState> rasterizer;
    ComPtr<ID3D11PixelShader> overlayPs;
    ComPtr<ID3D11BlendState> overlayBlend;
    ComPtr<ID3D11Texture2D> overlayTexture;
    ComPtr<ID3D11ShaderResourceView> overlaySrv;
    HDC overlayDc = nullptr;
    HBITMAP overlayBitmap = nullptr;
    HGDIOBJ oldBitmap = nullptr;
    HGDIOBJ oldFont = nullptr;
    std::uint32_t* overlayPixels = nullptr;
    std::string statusText;

    std::vector<std::uint32_t> sourcePacked =
        std::vector<std::uint32_t>(SOURCE_WIDTH * SOURCE_HEIGHT);
    std::vector<Float4> sourceFloats =
        std::vector<Float4>(SOURCE_WIDTH * SOURCE_HEIGHT);

    std::string adapterName;
    D3D_FEATURE_LEVEL featureLevel = D3D_FEATURE_LEVEL_9_1;
    double fps = 0.0;
    float cpuAverage = 0.0f;

    ~Preview() {
        if (overlayDc) {
            if (oldFont) SelectObject(overlayDc, oldFont);
            if (oldBitmap) SelectObject(overlayDc, oldBitmap);
            DeleteDC(overlayDc);
        }
        if (overlayBitmap) DeleteObject(overlayBitmap);
    }
    void CreateDeviceAndSwapChain(HWND hwnd) {
        DXGI_SWAP_CHAIN_DESC swapDesc{};
        swapDesc.BufferDesc.Width = CLIENT_WIDTH;
        swapDesc.BufferDesc.Height = CLIENT_HEIGHT;
        swapDesc.BufferDesc.RefreshRate.Numerator = 60;
        swapDesc.BufferDesc.RefreshRate.Denominator = 1;
        swapDesc.BufferDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
        swapDesc.SampleDesc.Count = 1;
        swapDesc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
        swapDesc.BufferCount = 2;
        swapDesc.OutputWindow = hwnd;
        swapDesc.Windowed = TRUE;
        swapDesc.SwapEffect = DXGI_SWAP_EFFECT_DISCARD;

        const D3D_FEATURE_LEVEL requested[] = {
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0
        };
        Check(D3D11CreateDeviceAndSwapChain(
            nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0,
            requested, ARRAYSIZE(requested), D3D11_SDK_VERSION,
            &swapDesc, swapChain.GetAddressOf(),
            device.GetAddressOf(), &featureLevel, context.GetAddressOf()),
            "D3D11CreateDeviceAndSwapChain(HARDWARE)");

        ComPtr<ID3D11Texture2D> backBuffer;
        Check(swapChain->GetBuffer(0, IID_PPV_ARGS(backBuffer.GetAddressOf())),
              "SwapChain::GetBuffer");
        Check(device->CreateRenderTargetView(
            backBuffer.Get(), nullptr, backBufferRtv.GetAddressOf()),
            "CreateRenderTargetView(backbuffer)");

        ComPtr<IDXGIDevice> dxgiDevice;
        Check(device.As(&dxgiDevice), "Query IDXGIDevice");
        ComPtr<IDXGIAdapter> adapter;
        Check(dxgiDevice->GetAdapter(adapter.GetAddressOf()), "GetAdapter");
        DXGI_ADAPTER_DESC desc{};
        Check(adapter->GetDesc(&desc), "GetAdapterDesc");

        char utf8[256]{};
        WideCharToMultiByte(CP_UTF8, 0, desc.Description, -1,
                            utf8, static_cast<int>(sizeof(utf8)), nullptr, nullptr);
        adapterName = utf8;
    }

    void CreateShaders(const std::filesystem::path& shaderDir) {
        const auto reductionVsBytes = ReadBinary(shaderDir / "VS_Fullscreen.cso");
        const auto tileBytes = ReadBinary(shaderDir / "PS_LuminanceTile.cso");
        const auto finalBytes = ReadBinary(shaderDir / "PS_LuminanceFinal.cso");
        const auto displayVsBytes = ReadBinary(shaderDir / "VS_Display.cso");
        const auto displayColorBytes = ReadBinary(shaderDir / "PS_DisplayColor.cso");
        const auto displayScalarBytes = ReadBinary(shaderDir / "PS_DisplayScalar.cso");
        const auto overlayBytes = ReadBinary(shaderDir / "PS_DisplayOverlay.cso");

        Check(device->CreateVertexShader(
            reductionVsBytes.data(), reductionVsBytes.size(), nullptr,
            reductionVs.GetAddressOf()), "CreateVertexShader(reduction)");
        Check(device->CreatePixelShader(
            tileBytes.data(), tileBytes.size(), nullptr, tilePs.GetAddressOf()),
            "CreatePixelShader(tile)");
        Check(device->CreatePixelShader(
            finalBytes.data(), finalBytes.size(), nullptr, finalPs.GetAddressOf()),
            "CreatePixelShader(final)");
        Check(device->CreateVertexShader(
            displayVsBytes.data(), displayVsBytes.size(), nullptr,
            displayVs.GetAddressOf()), "CreateVertexShader(display)");
        Check(device->CreatePixelShader(
            displayColorBytes.data(), displayColorBytes.size(), nullptr,
            displayColorPs.GetAddressOf()), "CreatePixelShader(display color)");
        Check(device->CreatePixelShader(
            displayScalarBytes.data(), displayScalarBytes.size(), nullptr,
            displayScalarPs.GetAddressOf()), "CreatePixelShader(display scalar)");
        Check(device->CreatePixelShader(
            overlayBytes.data(), overlayBytes.size(), nullptr, overlayPs.GetAddressOf()),
            "CreatePixelShader(overlay)");
    }

    void CreateStatesAndConstants() {
        D3D11_RASTERIZER_DESC rasterDesc{};
        rasterDesc.FillMode = D3D11_FILL_SOLID;
        rasterDesc.CullMode = D3D11_CULL_NONE;
        rasterDesc.DepthClipEnable = TRUE;
        Check(device->CreateRasterizerState(&rasterDesc, rasterizer.GetAddressOf()),
              "CreateRasterizerState");

        D3D11_SAMPLER_DESC samplerDesc{};
        samplerDesc.Filter = D3D11_FILTER_MIN_MAG_MIP_POINT;
        samplerDesc.AddressU = D3D11_TEXTURE_ADDRESS_CLAMP;
        samplerDesc.AddressV = D3D11_TEXTURE_ADDRESS_CLAMP;
        samplerDesc.AddressW = D3D11_TEXTURE_ADDRESS_CLAMP;
        samplerDesc.MaxLOD = D3D11_FLOAT32_MAX;
        Check(device->CreateSamplerState(&samplerDesc, pointSampler.GetAddressOf()),
              "CreateSamplerState");

        D3D11_BLEND_DESC blendDesc{};
        auto& overlayTarget = blendDesc.RenderTarget[0];
        overlayTarget.BlendEnable = TRUE;
        overlayTarget.SrcBlend = D3D11_BLEND_SRC_ALPHA;
        overlayTarget.DestBlend = D3D11_BLEND_INV_SRC_ALPHA;
        overlayTarget.BlendOp = D3D11_BLEND_OP_ADD;
        overlayTarget.SrcBlendAlpha = D3D11_BLEND_ONE;
        overlayTarget.DestBlendAlpha = D3D11_BLEND_INV_SRC_ALPHA;
        overlayTarget.BlendOpAlpha = D3D11_BLEND_OP_ADD;
        overlayTarget.RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL;
        Check(device->CreateBlendState(&blendDesc, overlayBlend.GetAddressOf()),
              "CreateBlendState(overlay)");
        D3D11_BUFFER_DESC cbDesc{};
        cbDesc.ByteWidth = 16;
        cbDesc.Usage = D3D11_USAGE_DEFAULT;
        cbDesc.BindFlags = D3D11_BIND_CONSTANT_BUFFER;
        const float zeros[4] = {};
        D3D11_SUBRESOURCE_DATA initial{};
        initial.pSysMem = zeros;

        Check(device->CreateBuffer(
            &cbDesc, &initial, luminanceConstants.GetAddressOf()),
            "CreateBuffer(luminance constants)");
        Check(device->CreateBuffer(
            &cbDesc, &initial, displayConstants.GetAddressOf()),
            "CreateBuffer(display constants)");
    }

    void CreateTextures() {
        D3D11_TEXTURE2D_DESC sourceDesc{};
        sourceDesc.Width = SOURCE_WIDTH;
        sourceDesc.Height = SOURCE_HEIGHT;
        sourceDesc.MipLevels = 1;
        sourceDesc.ArraySize = 1;
        sourceDesc.Format = DXGI_FORMAT_R11G11B10_FLOAT;
        sourceDesc.SampleDesc.Count = 1;
        sourceDesc.Usage = D3D11_USAGE_DEFAULT;
        sourceDesc.BindFlags = D3D11_BIND_SHADER_RESOURCE;

        D3D11_SUBRESOURCE_DATA sourceData{};
        sourceData.pSysMem = sourcePacked.data();
        sourceData.SysMemPitch = SOURCE_WIDTH * sizeof(std::uint32_t);
        Check(device->CreateTexture2D(
            &sourceDesc, &sourceData, sourceTexture.GetAddressOf()),
            "CreateTexture2D(R11G11B10 source)");
        Check(device->CreateShaderResourceView(
            sourceTexture.Get(), nullptr, sourceSrv.GetAddressOf()),
            "CreateShaderResourceView(source)");

        D3D11_TEXTURE2D_DESC partialDesc{};
        partialDesc.Width = PARTIAL_WIDTH;
        partialDesc.Height = PARTIAL_HEIGHT;
        partialDesc.MipLevels = 1;
        partialDesc.ArraySize = 1;
        partialDesc.Format = DXGI_FORMAT_R32_FLOAT;
        partialDesc.SampleDesc.Count = 1;
        partialDesc.Usage = D3D11_USAGE_DEFAULT;
        partialDesc.BindFlags =
            D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;

        Check(device->CreateTexture2D(
            &partialDesc, nullptr, partialTexture.GetAddressOf()),
            "CreateTexture2D(partial R32_FLOAT)");

        D3D11_RENDER_TARGET_VIEW_DESC rtvDesc{};
        rtvDesc.Format = DXGI_FORMAT_R32_FLOAT;
        rtvDesc.ViewDimension = D3D11_RTV_DIMENSION_TEXTURE2D;
        Check(device->CreateRenderTargetView(
            partialTexture.Get(), &rtvDesc, partialRtv.GetAddressOf()),
            "CreateRenderTargetView(partials)");

        D3D11_SHADER_RESOURCE_VIEW_DESC srvDesc{};
        srvDesc.Format = DXGI_FORMAT_R32_FLOAT;
        srvDesc.ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2D;
        srvDesc.Texture2D.MipLevels = 1;
        Check(device->CreateShaderResourceView(
            partialTexture.Get(), &srvDesc, partialSrv.GetAddressOf()),
            "CreateShaderResourceView(partials)");

        D3D11_TEXTURE2D_DESC finalDesc = partialDesc;
        finalDesc.Width = 1;
        finalDesc.Height = 1;
        Check(device->CreateTexture2D(
            &finalDesc, nullptr, finalTexture.GetAddressOf()),
            "CreateTexture2D(final R32_FLOAT)");
        Check(device->CreateRenderTargetView(
            finalTexture.Get(), &rtvDesc, finalRtv.GetAddressOf()),
            "CreateRenderTargetView(final)");
        Check(device->CreateShaderResourceView(
            finalTexture.Get(), &srvDesc, finalSrv.GetAddressOf()),
            "CreateShaderResourceView(final)");
    }

    void CreateOverlay() {
        BITMAPINFO bitmapInfo{};
        bitmapInfo.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
        bitmapInfo.bmiHeader.biWidth = CLIENT_WIDTH;
        bitmapInfo.bmiHeader.biHeight = -CLIENT_HEIGHT; // top-down DIB
        bitmapInfo.bmiHeader.biPlanes = 1;
        bitmapInfo.bmiHeader.biBitCount = 32;
        bitmapInfo.bmiHeader.biCompression = BI_RGB;

        HDC screenDc = GetDC(gWindow);
        overlayDc = CreateCompatibleDC(screenDc);
        void* pixels = nullptr;
        overlayBitmap = CreateDIBSection(
            screenDc, &bitmapInfo, DIB_RGB_COLORS, &pixels, nullptr, 0);
        ReleaseDC(gWindow, screenDc);
        if (!overlayDc || !overlayBitmap || !pixels)
            throw std::runtime_error("Could not create the GDI text overlay bitmap.");

        overlayPixels = static_cast<std::uint32_t*>(pixels);
        oldBitmap = SelectObject(overlayDc, overlayBitmap);
        oldFont = SelectObject(overlayDc, gLabelFont);
        SetBkMode(overlayDc, TRANSPARENT);
        SetTextColor(overlayDc, RGB(210, 220, 238));
        std::memset(overlayPixels, 0,
                    static_cast<std::size_t>(CLIENT_WIDTH) * CLIENT_HEIGHT * 4);

        D3D11_TEXTURE2D_DESC textureDesc{};
        textureDesc.Width = CLIENT_WIDTH;
        textureDesc.Height = CLIENT_HEIGHT;
        textureDesc.MipLevels = 1;
        textureDesc.ArraySize = 1;
        textureDesc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        textureDesc.SampleDesc.Count = 1;
        textureDesc.Usage = D3D11_USAGE_DEFAULT;
        textureDesc.BindFlags = D3D11_BIND_SHADER_RESOURCE;

        D3D11_SUBRESOURCE_DATA initial{};
        initial.pSysMem = overlayPixels;
        initial.SysMemPitch = CLIENT_WIDTH * 4;
        Check(device->CreateTexture2D(
            &textureDesc, &initial, overlayTexture.GetAddressOf()),
            "CreateTexture2D(overlay)");
        Check(device->CreateShaderResourceView(
            overlayTexture.Get(), nullptr, overlaySrv.GetAddressOf()),
            "CreateShaderResourceView(overlay)");
    }

    void UpdateOverlay(const char* title) {
        if (!overlayDc || !overlayPixels || !overlayTexture) return;

        std::memset(overlayPixels, 0,
                    static_cast<std::size_t>(CLIENT_WIDTH) * CLIENT_HEIGHT * 4);
        SetBkMode(overlayDc, TRANSPARENT);
        SetTextColor(overlayDc, RGB(206, 218, 238));

        auto drawText = [&](int x, int y, const char* text) {
            TextOutA(overlayDc, x, y, text, static_cast<int>(std::strlen(text)));
        };

        drawText(40, 34, title);
        drawText(PANEL_X[0], 112, "01  INPUT TEXTURE  /  R11G11B10_FLOAT");
        drawText(PANEL_X[1], 112, "02  TILE REDUCTION  /  R32_FLOAT");
        drawText(PANEL_X[2], 112, "03  GLOBAL AVERAGE  /  1x1 R32_FLOAT");
        drawText(40, 344, statusText.c_str());
        GdiFlush();

        const std::size_t pixelCount =
            static_cast<std::size_t>(CLIENT_WIDTH) * CLIENT_HEIGHT;
        for (std::size_t i = 0; i < pixelCount; ++i) {
            const std::uint32_t rgb = overlayPixels[i] & 0x00ffffffu;
            overlayPixels[i] = rgb ? (rgb | 0xff000000u) : 0u;
        }

        context->UpdateSubresource(
            overlayTexture.Get(), 0, nullptr, overlayPixels,
            CLIENT_WIDTH * 4, 0);
    }
    void SetViewport(float x, float y, float width, float height) {
        D3D11_VIEWPORT viewport{};
        viewport.TopLeftX = x;
        viewport.TopLeftY = y;
        viewport.Width = width;
        viewport.Height = height;
        viewport.MinDepth = 0.0f;
        viewport.MaxDepth = 1.0f;
        context->RSSetViewports(1, &viewport);
    }

    void UpdateSource(double timeSeconds) {
        constexpr float pi = 3.14159265358979323846f;
        for (UINT y = 0; y < SOURCE_HEIGHT; ++y) {
            for (UINT x = 0; x < SOURCE_WIDTH; ++x) {
                const float u = static_cast<float>(x) / (SOURCE_WIDTH - 1);
                const float v = static_cast<float>(y) / (SOURCE_HEIGHT - 1);
                const float t = static_cast<float>(timeSeconds);

                const float waveA = 0.5f + 0.5f *
                    std::sin(t * 1.15f + u * pi * 4.0f + v * pi * 1.5f);
                const float waveB = 0.5f + 0.5f *
                    std::sin(t * 0.82f - u * pi * 2.0f + v * pi * 3.0f);
                const float waveC = 0.5f + 0.5f *
                    std::cos(t * 0.63f + u * pi * 3.0f - v * pi * 2.0f);

                Float4 color{
                    0.06f + 0.82f * waveA,
                    0.04f + 0.78f * waveB,
                    0.03f + 0.72f * waveC,
                    1.0f
                };

                const std::size_t index =
                    static_cast<std::size_t>(y) * SOURCE_WIDTH + x;
                const std::uint32_t packed = PackR11G11B10(color);
                sourcePacked[index] = packed;
                sourceFloats[index] = UnpackR11G11B10(packed);
            }
        }

        cpuAverage = CpuAverage(sourceFloats);
        if (sourceTexture) {
            context->UpdateSubresource(sourceTexture.Get(), 0, nullptr, sourcePacked.data(),
                                    SOURCE_WIDTH * sizeof(std::uint32_t), 0);
        }
    }

    void RunReduction() {
        context->IASetInputLayout(nullptr);
        context->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        context->RSSetState(rasterizer.Get());
        context->OMSetBlendState(nullptr, nullptr, 0xffffffff);
        context->OMSetDepthStencilState(nullptr, 0);
        context->VSSetShader(reductionVs.Get(), nullptr, 0);

        const float stage1Constants[4] = {
            0.0f, 0.0f,
            static_cast<float>(SOURCE_WIDTH),
            static_cast<float>(SOURCE_HEIGHT)
        };
        context->UpdateSubresource(
            luminanceConstants.Get(), 0, nullptr, stage1Constants, 0, 0);
        ID3D11Buffer* cb = luminanceConstants.Get();
        context->PSSetConstantBuffers(12, 1, &cb);

        ID3D11ShaderResourceView* sourceView = sourceSrv.Get();
        context->PSSetShaderResources(3, 1, &sourceView);
        context->PSSetShader(tilePs.Get(), nullptr, 0);
        ID3D11RenderTargetView* partialTarget = partialRtv.Get();
        context->OMSetRenderTargets(1, &partialTarget, nullptr);
        SetViewport(0.0f, 0.0f,
                    static_cast<float>(PARTIAL_WIDTH),
                    static_cast<float>(PARTIAL_HEIGHT));
        context->Draw(3, 0);

        ID3D11ShaderResourceView* nullView = nullptr;
        context->PSSetShaderResources(3, 1, &nullView);
        context->OMSetRenderTargets(0, nullptr, nullptr);

        const float stage2Constants[4] = {
            0.0f, 0.0f,
            static_cast<float>(PARTIAL_WIDTH),
            static_cast<float>(PARTIAL_HEIGHT)
        };
        context->UpdateSubresource(
            luminanceConstants.Get(), 0, nullptr, stage2Constants, 0, 0);
        ID3D11ShaderResourceView* partialView = partialSrv.Get();
        context->PSSetShaderResources(2, 1, &partialView);
        context->PSSetShader(finalPs.Get(), nullptr, 0);
        ID3D11RenderTargetView* finalTarget = finalRtv.Get();
        context->OMSetRenderTargets(1, &finalTarget, nullptr);
        SetViewport(0.0f, 0.0f, 1.0f, 1.0f);
        context->Draw(3, 0);

        context->PSSetShaderResources(2, 1, &nullView);
        context->OMSetRenderTargets(0, nullptr, nullptr);
    }

    void DrawPanel(ID3D11ShaderResourceView* view, ID3D11PixelShader* shader,
                   int panelIndex, float scalarScale = 1.0f) {
        ID3D11ShaderResourceView* resource = view;
        context->PSSetShaderResources(0, 1, &resource);
        context->PSSetShader(shader, nullptr, 0);

        const float constants[4] = { scalarScale, 0.0f, 0.0f, 0.0f };
        context->UpdateSubresource(
            displayConstants.Get(), 0, nullptr, constants, 0, 0);
        ID3D11Buffer* cb = displayConstants.Get();
        context->PSSetConstantBuffers(0, 1, &cb);

        SetViewport(static_cast<float>(PANEL_X[panelIndex]),
                    static_cast<float>(PANEL_Y),
                    static_cast<float>(PANEL_WIDTH),
                    static_cast<float>(PANEL_HEIGHT));
        context->Draw(3, 0);

        ID3D11ShaderResourceView* nullView = nullptr;
        context->PSSetShaderResources(0, 1, &nullView);
    }

    void Render(double timeSeconds) {
        UpdateSource(timeSeconds);
        RunReduction();

        const float clearColor[4] = { 0.025f, 0.035f, 0.055f, 1.0f };
        context->OMSetRenderTargets(1, backBufferRtv.GetAddressOf(), nullptr);
        context->ClearRenderTargetView(backBufferRtv.Get(), clearColor);
        context->IASetInputLayout(nullptr);
        context->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        context->RSSetState(rasterizer.Get());
        context->VSSetShader(displayVs.Get(), nullptr, 0);
        ID3D11SamplerState* sampler = pointSampler.Get();
        context->PSSetSamplers(0, 1, &sampler);

        DrawPanel(sourceSrv.Get(), displayColorPs.Get(), 0);

        const float tileDisplayScale =
            static_cast<float>(SOURCE_WIDTH * SOURCE_HEIGHT) /
            static_cast<float>(TILE_SIZE * TILE_SIZE);
        DrawPanel(partialSrv.Get(), displayScalarPs.Get(), 1, tileDisplayScale);
        DrawPanel(finalSrv.Get(), displayScalarPs.Get(), 2, 1.0f);

        ID3D11ShaderResourceView* overlayView = overlaySrv.Get();
        context->PSSetShaderResources(0, 1, &overlayView);
        context->PSSetShader(overlayPs.Get(), nullptr, 0);
        const float blendFactor[4] = {};
        context->OMSetBlendState(overlayBlend.Get(), blendFactor, 0xffffffff);
        SetViewport(0.0f, 0.0f, static_cast<float>(CLIENT_WIDTH), static_cast<float>(CLIENT_HEIGHT));
        context->Draw(3, 0);
        ID3D11ShaderResourceView* nullView = nullptr;
        context->PSSetShaderResources(0, 1, &nullView);
        context->OMSetBlendState(nullptr, nullptr, 0xffffffff);

        context->OMSetRenderTargets(0, nullptr, nullptr);
        swapChain->Present(0, 0);
    }

    void UpdateTitle() {
        char title[256]{};
        std::snprintf(title, sizeof(title),
                      "GLUE5 / GM45 D3D11 Preview  |  FPS: %5.1f  |  CPU ref Y: %.4f",
                      fps, cpuAverage);
        SetWindowTextA(gWindow, title);
        UpdateOverlay(title);
    }

    void Run(const std::filesystem::path& shaderDir) {
        CreateDeviceAndSwapChain(gWindow);
        CreateShaders(shaderDir);
        CreateStatesAndConstants();
        UpdateSource(0.0);
        CreateTextures();
        CreateOverlay();

        char status[512]{};
        const char* gpuLabel = adapterName.find("Mobile Intel") != std::string::npos
            ? "GM45 / Intel 4 Series" : adapterName.c_str();
        std::snprintf(status, sizeof(status),
                      "GPU: %s  |  D3D FL: %s  |  SYNTHETIC INPUT",
                      gpuLabel,
                      featureLevel == D3D_FEATURE_LEVEL_10_0 ? "10_0" :
                      featureLevel == D3D_FEATURE_LEVEL_10_1 ? "10_1" : "11_x");
        statusText = status;

        LARGE_INTEGER frequency{}, previous{}, startCounter{};
        QueryPerformanceFrequency(&frequency);
        QueryPerformanceCounter(&previous);
        startCounter = previous;
        unsigned frames = 0;
        UpdateTitle();

        MSG message{};
        bool running = true;
        while (running) {
            while (PeekMessage(&message, nullptr, 0, 0, PM_REMOVE)) {
                if (message.message == WM_QUIT) {
                    running = false;
                    break;
                }
                TranslateMessage(&message);
                DispatchMessage(&message);
            }
            if (!running) break;

            if (IsIconic(gWindow)) {
                WaitMessage();
                QueryPerformanceCounter(&previous);
                frames = 0;
                continue;
            }

            LARGE_INTEGER now{};
            QueryPerformanceCounter(&now);
            const double timeSeconds = static_cast<double>(now.QuadPart - startCounter.QuadPart) /
                                       static_cast<double>(frequency.QuadPart);
            Render(timeSeconds);
            ++frames;

            QueryPerformanceCounter(&now);
            const double sampleSeconds =
                static_cast<double>(now.QuadPart - previous.QuadPart) /
                static_cast<double>(frequency.QuadPart);
            if (sampleSeconds >= 0.5) {
                fps = static_cast<double>(frames) / sampleSeconds;
                UpdateTitle();
                frames = 0;
                previous = now;
            }
        }
    }
};

} // namespace

int WINAPI WinMain(HINSTANCE instance, HINSTANCE, LPSTR, int showCommand) {
    try {
        gLabelBrush = CreateSolidBrush(RGB(22, 29, 44));
        gLabelFont = CreateFontA(
            16, 0, 0, 0, FW_SEMIBOLD, FALSE, FALSE, FALSE,
            DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY, FIXED_PITCH | FF_MODERN, "Consolas");

        WNDCLASSEXA windowClass{};
        windowClass.cbSize = sizeof(windowClass);
        windowClass.style = CS_HREDRAW | CS_VREDRAW;
        windowClass.lpfnWndProc = WindowProc;
        windowClass.hInstance = instance;
        windowClass.hCursor = LoadCursor(nullptr, IDC_ARROW);
        windowClass.hbrBackground = gLabelBrush;
        windowClass.lpszClassName = "Glue5Gm45PreviewWindow";
        if (!RegisterClassExA(&windowClass)) {
            throw std::runtime_error("RegisterClassEx failed.");
        }

        RECT windowRect{ 0, 0, CLIENT_WIDTH, CLIENT_HEIGHT };
        AdjustWindowRect(&windowRect,
                         WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                         FALSE);

        gWindow = CreateWindowExA(
            0, windowClass.lpszClassName,
            "GLUE5 / GM45 D3D11 Preview",
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT, CW_USEDEFAULT,
            windowRect.right - windowRect.left,
            windowRect.bottom - windowRect.top,
            nullptr, nullptr, instance, nullptr);
        if (!gWindow) throw std::runtime_error("CreateWindowEx failed.");

ShowWindow(gWindow, showCommand);
        UpdateWindow(gWindow);

        char modulePath[MAX_PATH]{};
        GetModuleFileNameA(nullptr, modulePath, MAX_PATH);
        const std::filesystem::path executableDir = std::filesystem::path(modulePath).parent_path();

        {
            Preview preview;
            preview.Run(executableDir / "compiled");
        } // Release the overlay DC before deleting its selected font.

        if (gLabelFont) DeleteObject(gLabelFont);
        if (gLabelBrush) DeleteObject(gLabelBrush);
        return 0;
    }
    catch (const std::exception& error) {
        MessageBoxA(nullptr, error.what(),
                    "GLUE5 GM45 D3D11 Preview â€” error",
                    MB_OK | MB_ICONERROR);
        if (gLabelFont) DeleteObject(gLabelFont);
        if (gLabelBrush) DeleteObject(gLabelBrush);
        return 1;
    }
}
