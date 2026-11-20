cmake_minimum_required(VERSION 3.22.1)
project(cs2_android_bridge C)
if(NOT DEFINED CS2_ANDROID_RUST_LIBS)
    set(CS2_ANDROID_RUST_LIBS "$ENV{CS2_ANDROID_RUST_LIBS}")
endif()
set(cs2_android_library "${CS2_ANDROID_RUST_LIBS}/${ANDROID_ABI}/libcs2_android.so")
if(NOT CS2_ANDROID_RUST_LIBS OR NOT EXISTS "${cs2_android_library}")
    message(FATAL_ERROR "Build the native libraries with scripts/build/android_workspace.py build --integration, then set cs2.nativeBuildDir in Android local.properties for Android Studio.")
endif()
add_library(cs2_android SHARED IMPORTED)
set_target_properties(cs2_android PROPERTIES
    IMPORTED_LOCATION "${cs2_android_library}"
    # Rust's cdylib has no SONAME; link by name so DT_NEEDED stays portable.
    IMPORTED_NO_SONAME TRUE)
add_library(cs2_android_bridge SHARED bridge.c)
target_compile_options(cs2_android_bridge PRIVATE -Wall -Wextra -Werror)
target_link_libraries(cs2_android_bridge cs2_android android log)
