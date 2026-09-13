/**
 * @file
 * @brief Verify the public symbolic configuration-parser ABI.
 */

#include <assert.h>
#include <stddef.h>
#include <stdint.h>

#include "rptadvradio/rptadvradio.h"

typedef enum rptadv_radio_result (*config_parser)(const char *text,
                                                  uint32_t *value);

struct parser_case {
  /** Symbolic configuration token passed through the public ABI. */
  const char *text;
  /** Expected stable numeric assignment. */
  uint32_t value;
};

/** @brief Verify all accepted spellings and failure preservation for one
 * parser. */
static void test_parser(config_parser parser, const struct parser_case *cases,
                        uint32_t count) {
  uint32_t value;
  uint32_t index;

  assert(parser != NULL);
  for (index = 0U; index < count; ++index) {
    value = UINT32_MAX;
    assert(parser(cases[index].text, &value) == RPTADV_RADIO_OK);
    assert(value == cases[index].value);
  }
  value = UINT32_C(0x5a5a5a5a);
  assert(parser("unknown", &value) == RPTADV_RADIO_INVALID_ARGUMENT);
  assert(value == UINT32_C(0x5a5a5a5a));
  assert(parser("no ", &value) == RPTADV_RADIO_INVALID_ARGUMENT);
  assert(value == UINT32_C(0x5a5a5a5a));
  assert(parser(NULL, &value) == RPTADV_RADIO_INVALID_ARGUMENT);
  assert(value == UINT32_C(0x5a5a5a5a));
  assert(parser("no", NULL) == RPTADV_RADIO_INVALID_ARGUMENT);
}

int main(void) {
  static const struct parser_case rx_audio_cases[] = {
      {"NO", RPTADV_RADIO_RX_AUDIO_DISABLED},
      {"SpEaKeR", RPTADV_RADIO_RX_AUDIO_SPEAKER},
      {"flat", RPTADV_RADIO_RX_AUDIO_FLAT},
  };
  static const struct parser_case carrier_cases[] = {
      {"NO", RPTADV_RADIO_CARRIER_DISABLED},
      {"Dsp", RPTADV_RADIO_CARRIER_DSP},
      {"VOX", RPTADV_RADIO_CARRIER_VOX},
      {"usb", RPTADV_RADIO_CARRIER_USB},
      {"UsbInvert", RPTADV_RADIO_CARRIER_USB_INVERTED},
      {"PP", RPTADV_RADIO_CARRIER_PARALLEL},
      {"pPiNvErT", RPTADV_RADIO_CARRIER_PARALLEL_INVERTED},
  };
  static const struct parser_case ctcss_cases[] = {
      {"NO", RPTADV_RADIO_CTCSS_DISABLED},
      {"usb", RPTADV_RADIO_CTCSS_USB},
      {"UsbInvert", RPTADV_RADIO_CTCSS_USB_INVERTED},
      {"DSP", RPTADV_RADIO_CTCSS_DSP},
      {"pp", RPTADV_RADIO_CTCSS_PARALLEL},
      {"PpInVeRt", RPTADV_RADIO_CTCSS_PARALLEL_INVERTED},
  };
  static const struct parser_case tone_off_cases[] = {
      {"NO", RPTADV_RADIO_TONE_OFF_NONE},
      {"CtCsS_PhAsE_ShIfT", RPTADV_RADIO_TONE_OFF_PHASE_SHIFT},
      {"ctcss_tone_remove", RPTADV_RADIO_TONE_OFF_TONE_REMOVE},
      {"CTCSS_TAIL_TONE", RPTADV_RADIO_TONE_OFF_TAIL_TONE},
  };
  const struct rptadv_radio_descriptor *descriptor = rptadv_radio_descriptor();

  assert(descriptor != NULL);
  assert(descriptor->struct_size == sizeof(*descriptor));
  test_parser(descriptor->radio_parse_rx_audio_mode, rx_audio_cases,
              (uint32_t)(sizeof(rx_audio_cases) / sizeof(rx_audio_cases[0])));
  test_parser(descriptor->radio_parse_carrier_source, carrier_cases,
              (uint32_t)(sizeof(carrier_cases) / sizeof(carrier_cases[0])));
  test_parser(descriptor->radio_parse_ctcss_source, ctcss_cases,
              (uint32_t)(sizeof(ctcss_cases) / sizeof(ctcss_cases[0])));
  test_parser(descriptor->radio_parse_tone_off_mode, tone_off_cases,
              (uint32_t)(sizeof(tone_off_cases) / sizeof(tone_off_cases[0])));
  return 0;
}
