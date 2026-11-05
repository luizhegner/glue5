#include <windows.h>
#include <d3d11.h>
#include <dxgi.h>
#include <wrl/client.h>
#include <iostream>

using Microsoft::WRL::ComPtr;

int main() {
    const D3D_FEATURE_LEVEL requested[] = {
        D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0
    };
    ComPtr<ID3D11Device> device;
    ComPtr<ID3D11DeviceContext> context;
    D3D_FEATURE_LEVEL obtained{};
    HRESULT hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0,
        requested, ARRAYSIZE(requested), D3D11_SDK_VERSION,
        device.GetAddressOf(), &obtained, context.GetAddressOf());
    if (FAILED(hr)) {
        std::cerr << "D3D11CreateDevice failed: 0x" << std::hex << hr << "\n";
        return 1;
    }

    UINT support = 0;
    hr = device->CheckFormatSupport(DXGI_FORMAT_R11G11B10_FLOAT, &support);
    std::cout << "D3D feature level: " << std::hex << obtained << std::dec << "\n";
    std::cout << "R11G11B10_FLOAT support mask: 0x" << std::hex << support << std::dec << "\n";
    if (FAILED(hr)) {
        std::cerr << "CheckFormatSupport failed: 0x" << std::hex << hr << "\n";
        return 2;
    }

    const UINT required = D3D11_FORMAT_SUPPORT_TEXTURE2D |
                          D3D11_FORMAT_SUPPORT_SHADER_LOAD;
    if ((support & required) != required) {
        std::cout << "RESULT: format cannot be used as a Texture2D shader-load SRV\n";
        return 3;
    }

    D3D11_TEXTURE2D_DESC desc{};
    desc.Width = 16;
    desc.Height = 16;
    desc.MipLevels = 1;
    desc.ArraySize = 1;
    desc.Format = DXGI_FORMAT_R11G11B10_FLOAT;
    desc.SampleDesc.Count = 1;
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.BindFlags = D3D11_BIND_SHADER_RESOURCE;

    ComPtr<ID3D11Texture2D> texture;
    hr = device->CreateTexture2D(&desc, nullptr, texture.GetAddressOf());
    if (FAILED(hr)) {
        std::cerr << "CreateTexture2D(R11G11B10_FLOAT) failed: 0x" << std::hex << hr << "\n";
        return 4;
    }

    ComPtr<ID3D11ShaderResourceView> srv;
    hr = device->CreateShaderResourceView(texture.Get(), nullptr, srv.GetAddressOf());
    if (FAILED(hr)) {
        std::cerr << "CreateShaderResourceView(R11G11B10_FLOAT) failed: 0x" << std::hex << hr << "\n";
        return 5;
    }

    std::cout << "RESULT: R11G11B10_FLOAT Texture2D + SRV creation PASS\n";
    return 0;
}
