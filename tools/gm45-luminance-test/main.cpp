#include <windows.h>
#include <d3d11.h>
#include <dxgi.h>
#include <wrl/client.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <limits>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

using Microsoft::WRL::ComPtr;

struct Float4 { float r, g, b, a; };

static void Check(HRESULT hr, const char* what) {
    if (FAILED(hr)) {
        std::ostringstream s;
        s << what << " failed: HRESULT=0x" << std::hex
          << static_cast<unsigned long>(hr);
        throw std::runtime_error(s.str());
    }
}

static const char* FeatureLevelName(D3D_FEATURE_LEVEL level) {
    switch (level) {
    case D3D_FEATURE_LEVEL_11_0: return "11_0";
    case D3D_FEATURE_LEVEL_10_1: return "10_1";
    case D3D_FEATURE_LEVEL_10_0: return "10_0";
    case D3D_FEATURE_LEVEL_9_3: return "9_3";
    case D3D_FEATURE_LEVEL_9_2: return "9_2";
    case D3D_FEATURE_LEVEL_9_1: return "9_1";
    default: return "unknown";
    }
}

static std::vector<std::uint8_t> ReadBinary(const std::filesystem::path& path) {
    std::ifstream file(path, std::ios::binary);
    if (!file) throw std::runtime_error("Cannot open shader bytecode: " + path.string());
    return std::vector<std::uint8_t>(
        std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>());
}

static void RequireFormat(ID3D11Device* device, DXGI_FORMAT format,
                          UINT required, const char* label) {
    UINT support = 0;
    Check(device->CheckFormatSupport(format, &support), "CheckFormatSupport");
    std::cout << label << " format support mask=0x" << std::hex << support
              << std::dec << "\n";
    if ((support & required) != required)
        throw std::runtime_error(std::string(label) + " lacks required D3D11 format support");
}

static D3D11_TEXTURE2D_DESC TextureDesc(UINT width, UINT height,
                                       DXGI_FORMAT format, UINT bindFlags) {
    D3D11_TEXTURE2D_DESC d{};
    d.Width = width;
    d.Height = height;
    d.MipLevels = 1;
    d.ArraySize = 1;
    d.Format = format;
    d.SampleDesc.Count = 1;
    d.Usage = D3D11_USAGE_DEFAULT;
    d.BindFlags = bindFlags;
    return d;
}

static void SetViewport(ID3D11DeviceContext* context, UINT width, UINT height) {
    D3D11_VIEWPORT vp{};
    vp.Width = static_cast<float>(width);
    vp.Height = static_cast<float>(height);
    vp.MinDepth = 0.0f;
    vp.MaxDepth = 1.0f;
    context->RSSetViewports(1, &vp);
}

static float CpuAverage(const std::vector<Float4>& pixels, UINT width, UINT height) {
    constexpr float wr = 0.21250000596046447754f;
    constexpr float wg = 0.71539998054504394531f;
    constexpr float wb = 0.0720999985933303833f;
    double sum = 0.0;
    for (const auto& p : pixels) {
        const float luminance = p.r * wr + p.g * wg + p.b * wb;
        if (std::isfinite(luminance))
            sum += static_cast<double>(luminance);
    }
    return static_cast<float>(sum / (static_cast<double>(width) * height));
}

static bool RunCase(ID3D11Device* device, ID3D11DeviceContext* context,
                    ID3D11VertexShader* vs, ID3D11PixelShader* psTile,
                    ID3D11PixelShader* psFinal, ID3D11RasterizerState* rasterizer,
                    ID3D11Buffer* constants,
                    UINT width, UINT height) {
    const UINT partialWidth = (width + 15) / 16;
    const UINT partialHeight = (height + 15) / 16;
    const std::size_t count = static_cast<std::size_t>(width) * height;

    std::vector<Float4> pixels(count);
    for (UINT y = 0; y < height; ++y) {
        for (UINT x = 0; x < width; ++x) {
            const float r = 0.05f + static_cast<float>((x * 13 + y * 7) % 97) / 128.0f;
            const float g = 0.03f + static_cast<float>((x * 3 + y * 17) % 83) / 144.0f;
            const float b = 0.02f + static_cast<float>((x * 19 + y * 5) % 71) / 160.0f;
            pixels[static_cast<std::size_t>(y) * width + x] = { r, g, b, 1.0f };
        }
    }
    if (count > 8) {
        pixels[0].r = std::numeric_limits<float>::quiet_NaN();
        pixels[1].g = std::numeric_limits<float>::infinity();
    }

    D3D11_TEXTURE2D_DESC srcDesc = TextureDesc(
        width, height, DXGI_FORMAT_R32G32B32A32_FLOAT, D3D11_BIND_SHADER_RESOURCE);
    D3D11_SUBRESOURCE_DATA initial{};
    initial.pSysMem = pixels.data();
    initial.SysMemPitch = width * sizeof(Float4);

    ComPtr<ID3D11Texture2D> sourceTexture;
    Check(device->CreateTexture2D(&srcDesc, &initial, sourceTexture.GetAddressOf()),
          "CreateTexture2D(source)");
    ComPtr<ID3D11ShaderResourceView> sourceSrv;
    Check(device->CreateShaderResourceView(sourceTexture.Get(), nullptr, sourceSrv.GetAddressOf()),
          "CreateShaderResourceView(source)");

    D3D11_TEXTURE2D_DESC partialDesc = TextureDesc(
        partialWidth, partialHeight, DXGI_FORMAT_R32_FLOAT,
        D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE);
    ComPtr<ID3D11Texture2D> partialTexture;
    Check(device->CreateTexture2D(&partialDesc, nullptr, partialTexture.GetAddressOf()),
          "CreateTexture2D(partials)");

    D3D11_RENDER_TARGET_VIEW_DESC partialRtvDesc{};
    partialRtvDesc.Format = DXGI_FORMAT_R32_FLOAT;
    partialRtvDesc.ViewDimension = D3D11_RTV_DIMENSION_TEXTURE2D;
    ComPtr<ID3D11RenderTargetView> partialRtv;
    Check(device->CreateRenderTargetView(partialTexture.Get(), &partialRtvDesc,
          partialRtv.GetAddressOf()), "CreateRenderTargetView(partials)");

    D3D11_SHADER_RESOURCE_VIEW_DESC partialSrvDesc{};
    partialSrvDesc.Format = DXGI_FORMAT_R32_FLOAT;
    partialSrvDesc.ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2D;
    partialSrvDesc.Texture2D.MipLevels = 1;
    ComPtr<ID3D11ShaderResourceView> partialSrv;
    Check(device->CreateShaderResourceView(partialTexture.Get(), &partialSrvDesc,
          partialSrv.GetAddressOf()), "CreateShaderResourceView(partials)");

    D3D11_TEXTURE2D_DESC finalDesc = TextureDesc(
        1, 1, DXGI_FORMAT_R32_FLOAT, D3D11_BIND_RENDER_TARGET);
    ComPtr<ID3D11Texture2D> finalTexture;
    Check(device->CreateTexture2D(&finalDesc, nullptr, finalTexture.GetAddressOf()),
          "CreateTexture2D(final)");

    D3D11_RENDER_TARGET_VIEW_DESC finalRtvDesc{};
    finalRtvDesc.Format = DXGI_FORMAT_R32_FLOAT;
    finalRtvDesc.ViewDimension = D3D11_RTV_DIMENSION_TEXTURE2D;
    ComPtr<ID3D11RenderTargetView> finalRtv;
    Check(device->CreateRenderTargetView(finalTexture.Get(), &finalRtvDesc,
          finalRtv.GetAddressOf()), "CreateRenderTargetView(final)");

    D3D11_TEXTURE2D_DESC stagingDesc = finalDesc;
    stagingDesc.Usage = D3D11_USAGE_STAGING;
    stagingDesc.BindFlags = 0;
    stagingDesc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    ComPtr<ID3D11Texture2D> staging;
    Check(device->CreateTexture2D(&stagingDesc, nullptr, staging.GetAddressOf()),
          "CreateTexture2D(staging)");

    context->IASetInputLayout(nullptr);
    context->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    context->VSSetShader(vs, nullptr, 0);
    context->RSSetState(rasterizer);
    context->OMSetBlendState(nullptr, nullptr, 0xffffffff);
    context->OMSetDepthStencilState(nullptr, 0);
    ID3D11Buffer* cb = constants;
    context->PSSetConstantBuffers(12, 1, &cb);

    // Stage 1: one pixel-shader invocation per 16x16 source tile.
    const float cbStage1[4] = { 0.0f, 0.0f,
                               static_cast<float>(width), static_cast<float>(height) };
    context->UpdateSubresource(constants, 0, nullptr, cbStage1, 0, 0);
    ID3D11ShaderResourceView* sourceView = sourceSrv.Get();
    context->PSSetShaderResources(3, 1, &sourceView);
    context->PSSetShader(psTile, nullptr, 0);
    ID3D11RenderTargetView* partialTarget = partialRtv.Get();
    context->OMSetRenderTargets(1, &partialTarget, nullptr);
    SetViewport(context, partialWidth, partialHeight);
    context->Draw(3, 0);

    // Remove the source binding and RTV before changing resource roles.
    ID3D11ShaderResourceView* nullView = nullptr;
    context->PSSetShaderResources(3, 1, &nullView);
    context->OMSetRenderTargets(0, nullptr, nullptr);

    // Stage 2: sum the partial grid into a single R32_FLOAT pixel.
    const float cbStage2[4] = { 0.0f, 0.0f,
                               static_cast<float>(partialWidth),
                               static_cast<float>(partialHeight) };
    context->UpdateSubresource(constants, 0, nullptr, cbStage2, 0, 0);
    ID3D11ShaderResourceView* partialView = partialSrv.Get();
    context->PSSetShaderResources(2, 1, &partialView);
    context->PSSetShader(psFinal, nullptr, 0);
    ID3D11RenderTargetView* finalTarget = finalRtv.Get();
    context->OMSetRenderTargets(1, &finalTarget, nullptr);
    SetViewport(context, 1, 1);
    context->Draw(3, 0);

    context->PSSetShaderResources(2, 1, &nullView);
    context->OMSetRenderTargets(0, nullptr, nullptr);
    context->CopyResource(staging.Get(), finalTexture.Get());
    context->Flush();

    D3D11_MAPPED_SUBRESOURCE mapped{};
    Check(context->Map(staging.Get(), 0, D3D11_MAP_READ, 0, &mapped),
          "Map(final readback)");
    const float actual = *static_cast<const float*>(mapped.pData);
    context->Unmap(staging.Get(), 0);

    const float expected = CpuAverage(pixels, width, height);
    const float error = std::fabs(actual - expected);
    const float tolerance = std::max(0.00005f, std::fabs(expected) * 0.0005f);

    std::cout << width << "x" << height << " -> "
              << partialWidth << "x" << partialHeight
              << " | GPU=" << std::setprecision(9) << actual
              << " CPU=" << expected
              << " abs_error=" << error
              << " tolerance=" << tolerance
              << " | " << (std::isfinite(actual) && error <= tolerance ? "PASS" : "FAIL")
              << "\n";
    return std::isfinite(actual) && error <= tolerance;
}

int main(int argc, char** argv) {
    try {
        const std::filesystem::path dir = argc > 1
            ? std::filesystem::path(argv[1])
            : std::filesystem::path(argv[0]).parent_path();

        const auto vsBytes = ReadBinary(dir / "VS_Fullscreen.cso");
        const auto tileBytes = ReadBinary(dir / "PS_LuminanceTile.cso");
        const auto finalBytes = ReadBinary(dir / "PS_LuminanceFinal.cso");

        const D3D_FEATURE_LEVEL requested[] = {
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0
        };

        ComPtr<ID3D11Device> device;
        ComPtr<ID3D11DeviceContext> context;
        D3D_FEATURE_LEVEL obtained{};
        Check(D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0,
              requested, ARRAYSIZE(requested), D3D11_SDK_VERSION,
              device.GetAddressOf(), &obtained, context.GetAddressOf()),
              "D3D11CreateDevice(HARDWARE)");

        std::cout << "D3D feature level: " << FeatureLevelName(obtained) << "\n";

        ComPtr<IDXGIDevice> dxgiDevice;
        Check(device.As(&dxgiDevice), "Query IDXGIDevice");
        ComPtr<IDXGIAdapter> adapter;
        Check(dxgiDevice->GetAdapter(adapter.GetAddressOf()), "GetAdapter");
        DXGI_ADAPTER_DESC adapterDesc{};
        Check(adapter->GetDesc(&adapterDesc), "GetAdapterDesc");
        std::wcout << L"Adapter: " << adapterDesc.Description
                   << L" | VendorId=0x" << std::hex << adapterDesc.VendorId
                   << L" DeviceId=0x" << adapterDesc.DeviceId << std::dec << L"\n";

        const UINT tex2dLoad = D3D11_FORMAT_SUPPORT_TEXTURE2D |
                               D3D11_FORMAT_SUPPORT_SHADER_LOAD;
        RequireFormat(device.Get(), DXGI_FORMAT_R32G32B32A32_FLOAT,
                      tex2dLoad, "RGBA32_FLOAT source");
        RequireFormat(device.Get(), DXGI_FORMAT_R32_FLOAT,
                      tex2dLoad | D3D11_FORMAT_SUPPORT_RENDER_TARGET,
                      "R32_FLOAT RTV/SRV");

        ComPtr<ID3D11VertexShader> vs;
        ComPtr<ID3D11PixelShader> psTile;
        ComPtr<ID3D11PixelShader> psFinal;
        Check(device->CreateVertexShader(vsBytes.data(), vsBytes.size(), nullptr,
              vs.GetAddressOf()), "CreateVertexShader");
        Check(device->CreatePixelShader(tileBytes.data(), tileBytes.size(), nullptr,
              psTile.GetAddressOf()), "CreatePixelShader(tile)");
        Check(device->CreatePixelShader(finalBytes.data(), finalBytes.size(), nullptr,
              psFinal.GetAddressOf()), "CreatePixelShader(final)");

        D3D11_RASTERIZER_DESC rasterDesc{};
        rasterDesc.FillMode = D3D11_FILL_SOLID;
        rasterDesc.CullMode = D3D11_CULL_NONE;
        rasterDesc.DepthClipEnable = TRUE;
        ComPtr<ID3D11RasterizerState> rasterizer;
        Check(device->CreateRasterizerState(&rasterDesc, rasterizer.GetAddressOf()),
              "CreateRasterizerState");

        D3D11_BUFFER_DESC cbDesc{};
        cbDesc.ByteWidth = 16;
        cbDesc.Usage = D3D11_USAGE_DEFAULT;
        cbDesc.BindFlags = D3D11_BIND_CONSTANT_BUFFER;
        const float initialConstants[4] = {};
        D3D11_SUBRESOURCE_DATA cbInitial{};
        cbInitial.pSysMem = initialConstants;
        ComPtr<ID3D11Buffer> constants;
        Check(device->CreateBuffer(&cbDesc, &cbInitial, constants.GetAddressOf()),
              "CreateBuffer(CB12)");

        bool ok = true;
        ok &= RunCase(device.Get(), context.Get(), vs.Get(), psTile.Get(),
                      psFinal.Get(), rasterizer.Get(), constants.Get(), 180, 81);
        ok &= RunCase(device.Get(), context.Get(), vs.Get(), psTile.Get(),
                      psFinal.Get(), rasterizer.Get(), constants.Get(), 240, 108);
        ok &= RunCase(device.Get(), context.Get(), vs.Get(), psTile.Get(),
                      psFinal.Get(), rasterizer.Get(), constants.Get(), 255, 241);

        std::cout << (ok ? "ALL GPU CASES PASSED" : "GPU VALIDATION FAILED") << "\n";
        return ok ? 0 : 2;
    }
    catch (const std::exception& e) {
        std::cerr << "ERROR: " << e.what() << "\n";
        return 1;
    }
}
