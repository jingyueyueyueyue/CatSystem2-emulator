cmake_minimum_required(VERSION 3.22.1)
project(cs2_android_bridge C)
if(NOT DEFINED ENV{CS2_ANDROID_RUST_LIBS})
    message(FATAL_ERROR "Use scripts/build/android_workspace.py after implementation is complete.")
endif()
add_library(cs2_android SHARED IMPORTED)
set_target_properties(cs2_android PROPERTIES
    IMPORTED_LOCATION "$ENV{CS2_ANDROID_RUST_LIBS}/${ANDROID_ABI}/libcs2_android.so"
    # Rust's cdylib has no SONAME; link by name so DT_NEEDED stays portable.
    IMPORTED_NO_SONAME TRUE)
add_library(cs2_android_bridge SHARED bridge.c)
target_compile_options(cs2_android_bridge PRIVATE -Wall -Wextra -Werror)
target_link_libraries(cs2_android_bridge cs2_android android log)
