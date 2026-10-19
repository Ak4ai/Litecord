// Prints the NVENC API layout the Rust encoder needs, as Rust constants.
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stddef.h>
#include "nvEncodeAPI.h" // from nv-codec-headers n12.1.14.0 (include/ffnvcodec)

#define SIZE(T) printf("pub const SIZE_%s: usize = %zu;\n", #T, sizeof(T))
#define OFF(T, f, name) printf("pub const %s: usize = %zu;\n", name, offsetof(T, f))

// A bitfield's byte offset and bit: set it on a zeroed struct and find the bit.
#define BIT(T, f, name) do { \
    static T s; memset(&s, 0, sizeof s); s.f = 1; \
    unsigned char *p = (unsigned char *)&s; size_t i; int b; \
    for (i = 0; i < sizeof s; i++) if (p[i]) break; \
    for (b = 0; b < 8; b++) if (p[i] & (1 << b)) break; \
    printf("pub const %s: (usize, u32) = (%zu, %d);\n", name, i, b); \
} while (0)

static void guid(const char *name, GUID g) {
    const unsigned char *p = (const unsigned char *)&g;
    printf("pub const %s: [u8; 16] = [", name);
    for (int i = 0; i < 16; i++) printf("%s0x%02x", i ? ", " : "", p[i]);
    printf("];\n");
}

int main(void) {
    printf("pub const NVENCAPI_VERSION: u32 = 0x%08x;\n", (unsigned)NVENCAPI_VERSION);
    printf("pub const FUNCTION_LIST_VER: u32 = 0x%08x;\n", (unsigned)NV_ENCODE_API_FUNCTION_LIST_VER);
    printf("pub const OPEN_SESSION_EX_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER);
    printf("pub const PRESET_CONFIG_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_PRESET_CONFIG_VER);
    printf("pub const CONFIG_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_CONFIG_VER);
    printf("pub const RC_PARAMS_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_RC_PARAMS_VER);
    printf("pub const INITIALIZE_PARAMS_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_INITIALIZE_PARAMS_VER);
    printf("pub const CREATE_BITSTREAM_BUFFER_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_CREATE_BITSTREAM_BUFFER_VER);
    printf("pub const REGISTER_RESOURCE_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_REGISTER_RESOURCE_VER);
    printf("pub const MAP_INPUT_RESOURCE_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_MAP_INPUT_RESOURCE_VER);
    printf("pub const PIC_PARAMS_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_PIC_PARAMS_VER);
    printf("pub const LOCK_BITSTREAM_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_LOCK_BITSTREAM_VER);
    printf("pub const RECONFIGURE_PARAMS_VER: u32 = 0x%08x;\n", (unsigned)NV_ENC_RECONFIGURE_PARAMS_VER);

    guid("CODEC_H264_GUID", NV_ENC_CODEC_H264_GUID);
    guid("PRESET_P1_GUID", NV_ENC_PRESET_P1_GUID);
    guid("H264_PROFILE_MAIN_GUID", NV_ENC_H264_PROFILE_MAIN_GUID);

    SIZE(NV_ENCODE_API_FUNCTION_LIST);
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncOpenEncodeSessionEx, "FN_OPEN_SESSION_EX");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncGetEncodePresetConfigEx, "FN_GET_PRESET_CONFIG_EX");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncInitializeEncoder, "FN_INITIALIZE");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncCreateBitstreamBuffer, "FN_CREATE_BITSTREAM");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncDestroyBitstreamBuffer, "FN_DESTROY_BITSTREAM");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncEncodePicture, "FN_ENCODE_PICTURE");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncLockBitstream, "FN_LOCK_BITSTREAM");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncUnlockBitstream, "FN_UNLOCK_BITSTREAM");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncMapInputResource, "FN_MAP_INPUT");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncUnmapInputResource, "FN_UNMAP_INPUT");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncDestroyEncoder, "FN_DESTROY_ENCODER");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncRegisterResource, "FN_REGISTER_RESOURCE");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncUnregisterResource, "FN_UNREGISTER_RESOURCE");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncReconfigureEncoder, "FN_RECONFIGURE");
    OFF(NV_ENCODE_API_FUNCTION_LIST, nvEncGetLastErrorString, "FN_GET_LAST_ERROR");

    SIZE(NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS);
    OFF(NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS, deviceType, "OPEN_DEVICE_TYPE");
    OFF(NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS, device, "OPEN_DEVICE");
    OFF(NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS, apiVersion, "OPEN_API_VERSION");

    SIZE(NV_ENC_PRESET_CONFIG);
    OFF(NV_ENC_PRESET_CONFIG, presetCfg, "PRESET_CFG");

    SIZE(NV_ENC_CONFIG);
    OFF(NV_ENC_CONFIG, profileGUID, "CFG_PROFILE_GUID");
    OFF(NV_ENC_CONFIG, gopLength, "CFG_GOP_LENGTH");
    OFF(NV_ENC_CONFIG, frameIntervalP, "CFG_FRAME_INTERVAL_P");
    OFF(NV_ENC_CONFIG, rcParams, "CFG_RC");
    OFF(NV_ENC_CONFIG, encodeCodecConfig, "CFG_CODEC");
    OFF(NV_ENC_RC_PARAMS, version, "RC_VERSION");
    OFF(NV_ENC_RC_PARAMS, rateControlMode, "RC_MODE");
    OFF(NV_ENC_RC_PARAMS, averageBitRate, "RC_AVG_BITRATE");
    OFF(NV_ENC_RC_PARAMS, maxBitRate, "RC_MAX_BITRATE");
    OFF(NV_ENC_RC_PARAMS, vbvBufferSize, "RC_VBV_BUFFER");
    OFF(NV_ENC_RC_PARAMS, vbvInitialDelay, "RC_VBV_INITIAL_DELAY");
    OFF(NV_ENC_CONFIG_H264, idrPeriod, "H264_IDR_PERIOD");
    OFF(NV_ENC_CONFIG_H264, h264VUIParameters, "H264_VUI");
    BIT(NV_ENC_CONFIG_H264, repeatSPSPPS, "H264_REPEAT_SPSPPS_BIT");
    BIT(NV_ENC_CONFIG_H264, outputAUD, "H264_OUTPUT_AUD_BIT");
    OFF(NV_ENC_CONFIG_H264_VUI_PARAMETERS, videoSignalTypePresentFlag, "VUI_SIGNAL_TYPE_PRESENT");
    OFF(NV_ENC_CONFIG_H264_VUI_PARAMETERS, videoFormat, "VUI_VIDEO_FORMAT");
    OFF(NV_ENC_CONFIG_H264_VUI_PARAMETERS, videoFullRangeFlag, "VUI_FULL_RANGE");
    OFF(NV_ENC_CONFIG_H264_VUI_PARAMETERS, colourDescriptionPresentFlag, "VUI_COLOUR_DESCRIPTION_PRESENT");
    OFF(NV_ENC_CONFIG_H264_VUI_PARAMETERS, colourPrimaries, "VUI_COLOUR_PRIMARIES");
    OFF(NV_ENC_CONFIG_H264_VUI_PARAMETERS, transferCharacteristics, "VUI_TRANSFER");
    OFF(NV_ENC_CONFIG_H264_VUI_PARAMETERS, colourMatrix, "VUI_MATRIX");

    SIZE(NV_ENC_INITIALIZE_PARAMS);
    OFF(NV_ENC_INITIALIZE_PARAMS, encodeGUID, "INIT_ENCODE_GUID");
    OFF(NV_ENC_INITIALIZE_PARAMS, presetGUID, "INIT_PRESET_GUID");
    OFF(NV_ENC_INITIALIZE_PARAMS, encodeWidth, "INIT_WIDTH");
    OFF(NV_ENC_INITIALIZE_PARAMS, encodeHeight, "INIT_HEIGHT");
    OFF(NV_ENC_INITIALIZE_PARAMS, darWidth, "INIT_DAR_WIDTH");
    OFF(NV_ENC_INITIALIZE_PARAMS, darHeight, "INIT_DAR_HEIGHT");
    OFF(NV_ENC_INITIALIZE_PARAMS, frameRateNum, "INIT_FRAME_RATE_NUM");
    OFF(NV_ENC_INITIALIZE_PARAMS, frameRateDen, "INIT_FRAME_RATE_DEN");
    OFF(NV_ENC_INITIALIZE_PARAMS, enableEncodeAsync, "INIT_ENCODE_ASYNC");
    OFF(NV_ENC_INITIALIZE_PARAMS, enablePTD, "INIT_ENABLE_PTD");
    OFF(NV_ENC_INITIALIZE_PARAMS, encodeConfig, "INIT_ENCODE_CONFIG");
    OFF(NV_ENC_INITIALIZE_PARAMS, maxEncodeWidth, "INIT_MAX_WIDTH");
    OFF(NV_ENC_INITIALIZE_PARAMS, maxEncodeHeight, "INIT_MAX_HEIGHT");
    OFF(NV_ENC_INITIALIZE_PARAMS, tuningInfo, "INIT_TUNING_INFO");

    SIZE(NV_ENC_CREATE_BITSTREAM_BUFFER);
    OFF(NV_ENC_CREATE_BITSTREAM_BUFFER, bitstreamBuffer, "CREATE_BS_BUFFER");

    SIZE(NV_ENC_REGISTER_RESOURCE);
    OFF(NV_ENC_REGISTER_RESOURCE, resourceType, "REG_RESOURCE_TYPE");
    OFF(NV_ENC_REGISTER_RESOURCE, width, "REG_WIDTH");
    OFF(NV_ENC_REGISTER_RESOURCE, height, "REG_HEIGHT");
    OFF(NV_ENC_REGISTER_RESOURCE, pitch, "REG_PITCH");
    OFF(NV_ENC_REGISTER_RESOURCE, subResourceIndex, "REG_SUBRESOURCE");
    OFF(NV_ENC_REGISTER_RESOURCE, resourceToRegister, "REG_RESOURCE");
    OFF(NV_ENC_REGISTER_RESOURCE, registeredResource, "REG_REGISTERED");
    OFF(NV_ENC_REGISTER_RESOURCE, bufferFormat, "REG_BUFFER_FORMAT");
    OFF(NV_ENC_REGISTER_RESOURCE, bufferUsage, "REG_BUFFER_USAGE");

    SIZE(NV_ENC_MAP_INPUT_RESOURCE);
    OFF(NV_ENC_MAP_INPUT_RESOURCE, registeredResource, "MAP_REGISTERED");
    OFF(NV_ENC_MAP_INPUT_RESOURCE, mappedResource, "MAP_MAPPED");
    OFF(NV_ENC_MAP_INPUT_RESOURCE, mappedBufferFmt, "MAP_BUFFER_FORMAT");

    SIZE(NV_ENC_PIC_PARAMS);
    OFF(NV_ENC_PIC_PARAMS, inputWidth, "PIC_INPUT_WIDTH");
    OFF(NV_ENC_PIC_PARAMS, inputHeight, "PIC_INPUT_HEIGHT");
    OFF(NV_ENC_PIC_PARAMS, inputPitch, "PIC_INPUT_PITCH");
    OFF(NV_ENC_PIC_PARAMS, encodePicFlags, "PIC_FLAGS");
    OFF(NV_ENC_PIC_PARAMS, frameIdx, "PIC_FRAME_IDX");
    OFF(NV_ENC_PIC_PARAMS, inputTimeStamp, "PIC_TIMESTAMP");
    OFF(NV_ENC_PIC_PARAMS, inputBuffer, "PIC_INPUT_BUFFER");
    OFF(NV_ENC_PIC_PARAMS, outputBitstream, "PIC_OUTPUT_BITSTREAM");
    OFF(NV_ENC_PIC_PARAMS, bufferFmt, "PIC_BUFFER_FORMAT");
    OFF(NV_ENC_PIC_PARAMS, pictureStruct, "PIC_STRUCT");

    SIZE(NV_ENC_LOCK_BITSTREAM);
    OFF(NV_ENC_LOCK_BITSTREAM, outputBitstream, "LOCK_OUTPUT_BITSTREAM");
    OFF(NV_ENC_LOCK_BITSTREAM, bitstreamSizeInBytes, "LOCK_SIZE");
    OFF(NV_ENC_LOCK_BITSTREAM, bitstreamBufferPtr, "LOCK_PTR");
    OFF(NV_ENC_LOCK_BITSTREAM, pictureType, "LOCK_PICTURE_TYPE");

    SIZE(NV_ENC_RECONFIGURE_PARAMS);
    OFF(NV_ENC_RECONFIGURE_PARAMS, reInitEncodeParams, "RECONF_INIT_PARAMS");
    BIT(NV_ENC_RECONFIGURE_PARAMS, resetEncoder, "RECONF_RESET_BIT");
    BIT(NV_ENC_RECONFIGURE_PARAMS, forceIDR, "RECONF_FORCE_IDR_BIT");

    printf("pub const PARAMS_RC_CBR: u32 = %d;\n", NV_ENC_PARAMS_RC_CBR);
    printf("pub const TUNING_ULTRA_LOW_LATENCY: u32 = %d;\n", NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY);
    printf("pub const DEVICE_TYPE_DIRECTX: u32 = %d;\n", NV_ENC_DEVICE_TYPE_DIRECTX);
    printf("pub const INPUT_RESOURCE_TYPE_DIRECTX: u32 = %d;\n", NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX);
    printf("pub const BUFFER_FORMAT_NV12: u32 = %d;\n", NV_ENC_BUFFER_FORMAT_NV12);
    printf("pub const INPUT_IMAGE: u32 = %d;\n", NV_ENC_INPUT_IMAGE);
    printf("pub const PIC_STRUCT_FRAME: u32 = %d;\n", NV_ENC_PIC_STRUCT_FRAME);
    printf("pub const PIC_FLAG_FORCEIDR: u32 = %d;\n", NV_ENC_PIC_FLAG_FORCEIDR);
    printf("pub const PIC_FLAG_OUTPUT_SPSPPS: u32 = %d;\n", NV_ENC_PIC_FLAG_OUTPUT_SPSPPS);
    printf("pub const PIC_TYPE_IDR: u32 = %d;\n", NV_ENC_PIC_TYPE_IDR);
    printf("pub const ERR_ENCODER_NEEDS_MORE_INPUT: u32 = %d;\n", NV_ENC_ERR_NEED_MORE_INPUT);
    return 0;
}
