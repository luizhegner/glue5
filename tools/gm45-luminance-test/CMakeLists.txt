cmake_minimum_required(VERSION 3.20)
project(gm45_luminance_test LANGUAGES CXX)

if(NOT WIN32)
    message(FATAL_ERROR "This validation harness currently requires Windows and D3D11.")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# Locate the Windows SDK's legacy HLSL compiler.
find_program(FXC_EXECUTABLE NAMES fxc.exe fxc)
if(NOT FXC_EXECUTABLE)
    file(GLOB _fxc_candidates
        "C:/Program Files (x86)/Windows Kits/10/bin/*/x64/fxc.exe")
    list(SORT _fxc_candidates COMPARE NATURAL ORDER DESCENDING)
    if(_fxc_candidates)
        list(GET _fxc_candidates 0 FXC_EXECUTABLE)
    endif()
endif()
if(NOT FXC_EXECUTABLE)
    message(FATAL_ERROR "fxc.exe not found. Install the Windows SDK or put fxc on PATH.")
endif()

set(SHADER_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/shaders/luminance_fallback.hlsl")
set(SHADER_OUTPUT_DIR "${CMAKE_CURRENT_BINARY_DIR}/compiled")
file(MAKE_DIRECTORY "${SHADER_OUTPUT_DIR}")

set(VS_BYTECODE "${SHADER_OUTPUT_DIR}/VS_Fullscreen.cso")
set(TILE_BYTECODE "${SHADER_OUTPUT_DIR}/PS_LuminanceTile.cso")
set(FINAL_BYTECODE "${SHADER_OUTPUT_DIR}/PS_LuminanceFinal.cso")

add_custom_command(
    OUTPUT "${VS_BYTECODE}"
    COMMAND "${FXC_EXECUTABLE}" /nologo /T vs_4_0 /E VS_Fullscreen /Fo "${VS_BYTECODE}" "${SHADER_SOURCE}"
    DEPENDS "${SHADER_SOURCE}"
    VERBATIM
)
add_custom_command(
    OUTPUT "${TILE_BYTECODE}"
    COMMAND "${FXC_EXECUTABLE}" /nologo /T ps_4_0 /E PS_LuminanceTile /Fo "${TILE_BYTECODE}" "${SHADER_SOURCE}"
    DEPENDS "${SHADER_SOURCE}"
    VERBATIM
)
add_custom_command(
    OUTPUT "${FINAL_BYTECODE}"
    COMMAND "${FXC_EXECUTABLE}" /nologo /T ps_4_0 /E PS_LuminanceFinal /Fo "${FINAL_BYTECODE}" "${SHADER_SOURCE}"
    DEPENDS "${SHADER_SOURCE}"
    VERBATIM
)

add_custom_target(gm45_shaders DEPENDS
    "${VS_BYTECODE}" "${TILE_BYTECODE}" "${FINAL_BYTECODE}")

function(add_gm45_test target source)
    add_executable(${target} "${source}")
    target_compile_features(${target} PRIVATE cxx_std_17)
    target_compile_definitions(${target} PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX)
    target_link_libraries(${target} PRIVATE d3d11 dxgi)
    if(MSVC)
        target_compile_options(${target} PRIVATE /W4 /permissive-)
    endif()
    add_dependencies(${target} gm45_shaders)
    add_custom_command(TARGET ${target} POST_BUILD
        COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${VS_BYTECODE}" "${TILE_BYTECODE}" "${FINAL_BYTECODE}"
            "$<TARGET_FILE_DIR:${target}>"
        VERBATIM
    )
endfunction()

add_gm45_test(gm45_luminance_test main.cpp)
add_gm45_test(r11_format_probe r11_format_probe.cpp)
add_gm45_test(r11_luminance_test r11_luminance_test.cpp)

enable_testing()
add_test(NAME gm45_luminance_rgba32 COMMAND gm45_luminance_test)
add_test(NAME gm45_r11_format COMMAND r11_format_probe)
add_test(NAME gm45_luminance_r11g11b10 COMMAND r11_luminance_test)
