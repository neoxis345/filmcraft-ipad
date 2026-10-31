// The Rust half of FilmCraft for iPadOS (apps/filmcraft-ios, src/ffi.rs), as Swift sees it.
// Every function is called on the main thread; strings are UTF-8 and copied by the callee.

#ifndef FILMCRAFT_BRIDGING_HEADER_H
#define FILMCRAFT_BRIDGING_HEADER_H

#include <stdbool.h>
#include <stdint.h>

typedef struct FcHost FcHost;

FcHost *fc_host_create(void *metal_layer, uint32_t width, uint32_t height, float scale, const char *documents, const char *languages_json, uint64_t ram_bytes);
void fc_host_destroy(FcHost *host);
double fc_host_frame(FcHost *host);
void fc_host_resize(FcHost *host, uint32_t width, uint32_t height, float scale);
void fc_host_touch(FcHost *host, uint64_t id, int32_t phase, float x, float y, int32_t kind, bool secondary);
void fc_host_hover(FcHost *host, float x, float y, bool inside);
void fc_host_scroll(FcHost *host, float dx, float dy, int32_t phase);
void fc_host_pinch(FcHost *host, float factor, float x, float y);
void fc_host_key(FcHost *host, uint32_t usage, bool pressed, bool repeat, uint64_t flags, const char *chars, const char *unmodified);
void fc_host_text(FcHost *host, const char *text);
void fc_host_backspace(FcHost *host);
void fc_host_paste(FcHost *host, const char *text);
void fc_host_command(FcHost *host, const char *command_id);
void fc_host_open_paths(FcHost *host, const char *paths_json);
void fc_dialog_answer(uint64_t request, const char *paths_json);
void fc_host_memory_warning(FcHost *host);
void fc_host_thermal(FcHost *host, int32_t state);
void fc_host_low_power(FcHost *host, bool on);
void fc_host_set_active(FcHost *host, bool active);

#endif
