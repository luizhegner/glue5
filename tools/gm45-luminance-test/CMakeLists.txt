cmake_minimum_required(VERSION 3.20)
project(gm45_luminance_test LANGUAGES CXX)

if(NOT WIN32)
    message(FATAL_ERROR "These validation tools require Windows and D3D11.")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

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

set(SHADER_OUTPUT_DIR "${CMAKE_CURRENT_BINARY_DIR}/compiled")
file(MAKE_DIRECTORY "${SHADER_OUTPUT_DIR}")

set(REDUCTION_SHADER "${CMAKE_CURRENT_SOURCE_DIR}/shaders/luminance_fallback.hlsl")
set(DISPLAY_SHADER "${CMAKE_CURRENT_SOURCE_DIR}/shaders/preview_display.hlsl")

set(VS_BYTECODE "${SHADER_OUTPUT_DIR}/VS_Fullscreen.cso")
set(TILE_BYTECODE "${SHADER_OUTPUT_DIR}/PS_LuminanceTile.cso")
set(FINAL_BYTECODE "${SHADER_OUTPUT_DIR}/PS_LuminanceFinal.cso")
set(DISPLAY_VS_BYTECODE "${SHADER_OUTPUT_DIR}/VS_Display.cso")
set(DISPLAY_COLOR_BYTECODE "${SHADER_OUTPUT_DIR}/PS_DisplayColor.cso")
set(DISPLAY_SCALAR_BYTECODE "${SHADER_OUTPUT_DIR}/PS_DisplayScalar.cso")
set(DISPLAY_OVERLAY_BYTECODE "${SHADER_OUTPUT_DIR}/PS_DisplayOverlay.cso")

add_custom_command(OUTPUT "${VS_BYTECODE}"
    COMMAND "${FXC_EXECUTABLE}" /nologo /T vs_4_0 /E VS_Fullscreen /Fo "${VS_BYTECODE}" "${REDUCTION_SHADER}"
    DEPENDS "${REDUCTION_SHADER}" VERBATIM)
add_custom_command(OUTPUT "${TILE_BYTECODE}"
    COMMAND "${FXC_EXECUTABLE}" /nologo /T ps_4_0 /E PS_LuminanceTile /Fo "${TILE_BYTECODE}" "${REDUCTION_SHADER}"
    DEPENDS "${REDUCTION_SHADER}" VERBATIM)
add_custom_command(OUTPUT "${FINAL_BYTECODE}"
    COMMAND "${FXC_EXECUTABLE}" /nologo /T ps_4_0 /E PS_LuminanceFinal /Fo "${FINAL_BYTECODE}" "${REDUCTION_SHADER}"
    DEPENDS "${REDUCTION_SHADER}" VERBATIM)
add_custom_command(OUTPUT "${DISPLAY_VS_BYTECODE}"
    COMMAND "${FXC_EXECUTABLE}" /nologo /T vs_4_0 /E VS_Display /Fo "${DISPLAY_VS_BYTECODE}" "${DISPLAY_SHADER}"
    DEPENDS "${DISPLAY_SHADER}" VERBATIM)
add_custom_command(OUTPUT "${DISPLAY_COLOR_BYTECODE}"
    COMMAND "${FXC_EXECUTABLE}" /nologo /T ps_4_0 /E PS_DisplayColor /Fo "${DISPLAY_COLOR_BYTECODE}" "${DISPLAY_SHADER}"
    DEPENDS "${DISPLAY_SHADER}" VERBATIM)
add_custom_command(OUTPUT "${DISPLAY_SCALAR_BYTECODE}"
    COMMAND "${FXC_EXECUTABLE}" /nologo /T ps_4_0 /E PS_DisplayScalar /Fo "${DISPLAY_SCALAR_BYTECODE}" "${DISPLAY_SHADER}"
    DEPENDS "${DISPLAY_SHADER}" VERBATIM)
add_custom_command(OUTPUT "${DISPLAY_OVERLAY_BYTECODE}"
    COMMAND "${FXC_EXECUTABLE}" /nologo /T ps_4_0 /E PS_DisplayOverlay /Fo "${DISPLAY_OVERLAY_BYTECODE}" "${DISPLAY_SHADER}"
    DEPENDS "${DISPLAY_SHADER}" VERBATIM)

add_custom_target(gm45_shaders DEPENDS
    "${VS_BYTECODE}"
    "${TILE_BYTECODE}"
    "${FINAL_BYTECODE}"
    "${DISPLAY_VS_BYTECODE}"
    "${DISPLAY_COLOR_BYTECODE}"
    "${DISPLAY_SCALAR_BYTECODE}"
    "${DISPLAY_OVERLAY_BYTECODE}"
)

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
        VERBATIM)
endfunction()

add_gm45_test(gm45_luminance_test main.cpp)
add_gm45_test(r11_format_probe r11_format_probe.cpp)
add_gm45_test(r11_luminance_test r11_luminance_test.cpp)

add_executable(gm45_preview WIN32 gm45_preview.cpp)
target_compile_features(gm45_preview PRIVATE cxx_std_17)
target_compile_definitions(gm45_preview PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX)
target_link_libraries(gm45_preview PRIVATE d3d11 dxgi user32 gdi32)
if(MSVC)
    target_compile_options(gm45_preview PRIVATE /W4 /permissive-)
endif()
add_dependencies(gm45_preview gm45_shaders)
add_custom_command(TARGET gm45_preview POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E make_directory "$<TARGET_FILE_DIR:gm45_preview>/compiled"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${VS_BYTECODE}" "${TILE_BYTECODE}" "${FINAL_BYTECODE}"
        "${DISPLAY_VS_BYTECODE}" "${DISPLAY_COLOR_BYTECODE}"
        "${DISPLAY_SCALAR_BYTECODE}" "${DISPLAY_OVERLAY_BYTECODE}"
        "$<TARGET_FILE_DIR:gm45_preview>/compiled"
    VERBATIM)

enable_testing()
add_test(NAME gm45_luminance_rgba32 COMMAND gm45_luminance_test)
add_test(NAME gm45_r11_format COMMAND r11_format_probe)
add_test(NAME gm45_luminance_r11g11b10 COMMAND r11_luminance_test)
