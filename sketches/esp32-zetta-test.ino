/*
 * Zetta ESP32 Serial Console Test
 *
 * Intended for an ESP32 development board with an onboard USB-to-UART bridge.
 *
 * Serial settings:
 *   115200 baud
 *   8 data bits
 *   no parity
 *   1 stop bit
 *   no flow control
 *
 * Commands:
 *   test    Run interactive input/output test
 *   echo    Enter byte-for-byte echo mode
 *   ascii   Print the printable ASCII character set
 *   stress  Send a larger deterministic output stream
 *   status  Show test results
 *   help    Show commands
 *   reboot  Restart the ESP32
 *
 * In echo mode, enter a line containing only:
 *   /exit
 */

#include <Arduino.h>

static constexpr uint32_t SERIAL_BAUD = 115200;
static constexpr size_t INPUT_BUFFER_SIZE = 256;

enum class Mode {
  COMMAND,
  INTERACTIVE_TEST,
  ECHO
};

enum class TestStage {
  IDLE,
  BASIC_TEXT,
  CASE_SENSITIVE,
  SYMBOLS,
  LONG_LINE
};

struct TestResults {
  bool outputTestRun = false;
  bool basicTextPassed = false;
  bool caseSensitivePassed = false;
  bool symbolsPassed = false;
  bool longLinePassed = false;

  uint32_t linesReceived = 0;
  uint32_t bytesReceived = 0;
  uint32_t overflows = 0;

  bool allPassed() const {
    return outputTestRun &&
           basicTextPassed &&
           caseSensitivePassed &&
           symbolsPassed &&
           longLinePassed;
  }
};

Mode currentMode = Mode::COMMAND;
TestStage testStage = TestStage::IDLE;
TestResults results;

char inputBuffer[INPUT_BUFFER_SIZE];
size_t inputLength = 0;

bool previousWasCR = false;
const char *lastLineEnding = "none";

uint32_t bootNumber = 0;
uint32_t challengeNumber = 0;

const char *LONG_TEST_STRING =
    "0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ-abcdefghijklmnopqrstuvwxyz-"
    "The_quick_brown_fox_jumps_over_the_lazy_dog-9876543210";

void printSeparator() {
  Serial.println(F("------------------------------------------------------------"));
}

void printPrompt() {
  if (currentMode == Mode::COMMAND) {
    Serial.print(F("zetta-test> "));
  }
}

void printHelp() {
  printSeparator();
  Serial.println(F("Zetta serial-console test commands"));
  printSeparator();
  Serial.println(F("  test    Interactive input/output validation"));
  Serial.println(F("  echo    Byte-for-byte interactive echo test"));
  Serial.println(F("  ascii   Printable ASCII output test"));
  Serial.println(F("  stress  Bulk output and sequence test"));
  Serial.println(F("  status  Current test results"));
  Serial.println(F("  clear   Clear accumulated test results"));
  Serial.println(F("  help    Show this help"));
  Serial.println(F("  reboot  Restart the ESP32"));
  Serial.println();
  Serial.println(F("Serial configuration: 115200 8N1, no flow control"));
  printSeparator();
}

void printEscaped(const char *text) {
  Serial.print('"');

  for (size_t i = 0; text[i] != '\0'; ++i) {
    const uint8_t value = static_cast<uint8_t>(text[i]);

    switch (value) {
      case '\r':
        Serial.print(F("\\r"));
        break;

      case '\n':
        Serial.print(F("\\n"));
        break;

      case '\t':
        Serial.print(F("\\t"));
        break;

      case '\\':
        Serial.print(F("\\\\"));
        break;

      case '"':
        Serial.print(F("\\\""));
        break;

      default:
        if (value >= 32 && value <= 126) {
          Serial.write(value);
        } else {
          Serial.printf("\\x%02X", value);
        }
        break;
    }
  }

  Serial.print('"');
}

void reportComparison(const char *expected, const char *received) {
  Serial.print(F("Expected: "));
  printEscaped(expected);
  Serial.println();

  Serial.print(F("Received: "));
  printEscaped(received);
  Serial.println();

  const size_t expectedLength = strlen(expected);
  const size_t receivedLength = strlen(received);

  Serial.printf(
      "Expected length: %u byte(s)\r\n",
      static_cast<unsigned int>(expectedLength)
  );

  Serial.printf(
      "Received length: %u byte(s)\r\n",
      static_cast<unsigned int>(receivedLength)
  );

  size_t mismatch = 0;
  const size_t commonLength = min(expectedLength, receivedLength);

  while (mismatch < commonLength &&
         expected[mismatch] == received[mismatch]) {
    ++mismatch;
  }

  if (mismatch < commonLength) {
    Serial.printf(
        "First mismatch at byte %u: expected 0x%02X, received 0x%02X\r\n",
        static_cast<unsigned int>(mismatch),
        static_cast<unsigned int>(
            static_cast<uint8_t>(expected[mismatch])
        ),
        static_cast<unsigned int>(
            static_cast<uint8_t>(received[mismatch])
        )
    );
  } else if (expectedLength != receivedLength) {
    Serial.printf(
        "Content matches through byte %u, but the lengths differ.\r\n",
        static_cast<unsigned int>(commonLength)
    );
  }
}

bool checkAnswer(const char *expected, const char *received) {
  if (strcmp(expected, received) == 0) {
    Serial.println(F("[PASS] Input matched exactly."));
    return true;
  }

  Serial.println(F("[FAIL] Input did not match."));
  reportComparison(expected, received);
  return false;
}

void beginInteractiveTest() {
  currentMode = Mode::INTERACTIVE_TEST;
  testStage = TestStage::BASIC_TEXT;

  challengeNumber =
      static_cast<uint32_t>(esp_random() % 900000UL) + 100000UL;

  printSeparator();
  Serial.println(F("INTERACTIVE SERIAL TEST"));
  printSeparator();
  Serial.println(F("Zetta must have local echo disabled."));
  Serial.println(F("Enter each requested value exactly, then press Enter."));
  Serial.println();
  Serial.println(F("Test 1 of 4: basic text"));
  Serial.println(F("Type exactly: ZETTA SERIAL TEST"));
  Serial.print(F("> "));
}

void finishInteractiveTest() {
  currentMode = Mode::COMMAND;
  testStage = TestStage::IDLE;

  printSeparator();
  Serial.println(F("INTERACTIVE TEST COMPLETE"));
  printSeparator();

  Serial.printf(
      "Basic text:       %s\r\n",
      results.basicTextPassed ? "PASS" : "FAIL"
  );

  Serial.printf(
      "Case-sensitive:   %s\r\n",
      results.caseSensitivePassed ? "PASS" : "FAIL"
  );

  Serial.printf(
      "Symbols:          %s\r\n",
      results.symbolsPassed ? "PASS" : "FAIL"
  );

  Serial.printf(
      "Long line:        %s\r\n",
      results.longLinePassed ? "PASS" : "FAIL"
  );

  Serial.printf("Line ending seen: %s\r\n", lastLineEnding);

  printSeparator();

  if (results.basicTextPassed &&
      results.caseSensitivePassed &&
      results.symbolsPassed &&
      results.longLinePassed) {
    Serial.println(F("RESULT: PASS"));
    Serial.println(
        F("Bidirectional serial input and output are working correctly.")
    );
  } else {
    Serial.println(F("RESULT: FAIL"));
    Serial.println(
        F("One or more received strings differed from what was expected.")
    );
  }

  printSeparator();
  printPrompt();
}

void processInteractiveTestLine(const char *line) {
  char expected[96];

  switch (testStage) {
    case TestStage::BASIC_TEXT:
      results.basicTextPassed =
          checkAnswer("ZETTA SERIAL TEST", line);

      Serial.println();
      Serial.println(F("Test 2 of 4: case and numeric challenge"));
      snprintf(
          expected,
          sizeof(expected),
          "Zetta-%06lu-aBcDeF",
          static_cast<unsigned long>(challengeNumber)
      );
      Serial.print(F("Type exactly: "));
      Serial.println(expected);
      Serial.print(F("> "));

      testStage = TestStage::CASE_SENSITIVE;
      break;

    case TestStage::CASE_SENSITIVE:
      snprintf(
          expected,
          sizeof(expected),
          "Zetta-%06lu-aBcDeF",
          static_cast<unsigned long>(challengeNumber)
      );

      results.caseSensitivePassed = checkAnswer(expected, line);

      Serial.println();
      Serial.println(F("Test 3 of 4: punctuation"));
      Serial.println(F("Type exactly: []{}()<>!?@#$%^&*_-+=:;,."));
      Serial.print(F("> "));

      testStage = TestStage::SYMBOLS;
      break;

    case TestStage::SYMBOLS:
      results.symbolsPassed =
          checkAnswer("[]{}()<>!?@#$%^&*_-+=:;,.", line);

      Serial.println();
      Serial.println(F("Test 4 of 4: long line"));
      Serial.println(F("Type or paste exactly this line:"));
      Serial.println(LONG_TEST_STRING);
      Serial.print(F("> "));

      testStage = TestStage::LONG_LINE;
      break;

    case TestStage::LONG_LINE:
      results.longLinePassed =
          checkAnswer(LONG_TEST_STRING, line);

      finishInteractiveTest();
      break;

    default:
      finishInteractiveTest();
      break;
  }
}

void printAsciiTest() {
  results.outputTestRun = true;

  printSeparator();
  Serial.println(F("PRINTABLE ASCII OUTPUT TEST"));
  printSeparator();

  for (int row = 0; row < 6; ++row) {
    const int first = 32 + row * 16;
    const int last = min(first + 15, 126);

    Serial.printf("%02X-%02X  ", first, last);

    for (int value = first; value <= last; ++value) {
      Serial.write(static_cast<uint8_t>(value));
    }

    Serial.println();
  }

  Serial.println();
  Serial.println(F("Expected final character: ~"));
  Serial.println(F("[OUTPUT TEST COMPLETE]"));
  printSeparator();
}

uint32_t lineChecksum(const char *text) {
  uint32_t hash = 2166136261UL;

  while (*text != '\0') {
    hash ^= static_cast<uint8_t>(*text++);
    hash *= 16777619UL;
  }

  return hash;
}

void printStressTest() {
  results.outputTestRun = true;

  constexpr int lineCount = 500;
  char payload[96];

  printSeparator();
  Serial.println(F("STRESS TEST START"));
  Serial.printf("Lines: %d\r\n", lineCount);
  Serial.println(F("Every line contains a sequence number and checksum."));
  printSeparator();

  const uint32_t started = millis();

  for (int line = 0; line < lineCount; ++line) {
    snprintf(
        payload,
        sizeof(payload),
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ-abcdefghijklmnopqrstuvwxyz-0123456789-%04d",
        line
    );

    Serial.printf(
        "SEQ=%04d DATA=%s CRC=%08lX\r\n",
        line,
        payload,
        static_cast<unsigned long>(lineChecksum(payload))
    );

    /*
     * Allow the USB-UART transmit buffer and watchdog to make progress.
     * This does not intentionally throttle every individual byte.
     */
    if ((line % 25) == 0) {
      delay(1);
    }
  }

  Serial.flush();

  const uint32_t elapsed = millis() - started;

  printSeparator();
  Serial.printf(
      "STRESS TEST END: %d lines in %lu ms\r\n",
      lineCount,
      static_cast<unsigned long>(elapsed)
  );
  Serial.println(F("Expected final sequence number: 0499"));
  printSeparator();
}

void printStatus() {
  printSeparator();
  Serial.println(F("TEST STATUS"));
  printSeparator();

  Serial.printf(
      "ASCII/stress output run: %s\r\n",
      results.outputTestRun ? "YES" : "NO"
  );

  Serial.printf(
      "Basic text input:        %s\r\n",
      results.basicTextPassed ? "PASS" : "NOT PASSED"
  );

  Serial.printf(
      "Case-sensitive input:    %s\r\n",
      results.caseSensitivePassed ? "PASS" : "NOT PASSED"
  );

  Serial.printf(
      "Symbol input:            %s\r\n",
      results.symbolsPassed ? "PASS" : "NOT PASSED"
  );

  Serial.printf(
      "Long-line input:         %s\r\n",
      results.longLinePassed ? "PASS" : "NOT PASSED"
  );

  Serial.printf(
      "Lines received:          %lu\r\n",
      static_cast<unsigned long>(results.linesReceived)
  );

  Serial.printf(
      "Bytes received:          %lu\r\n",
      static_cast<unsigned long>(results.bytesReceived)
  );

  Serial.printf(
      "Input buffer overflows:  %lu\r\n",
      static_cast<unsigned long>(results.overflows)
  );

  Serial.printf("Last line ending:        %s\r\n", lastLineEnding);

  printSeparator();

  Serial.printf(
      "OVERALL RESULT: %s\r\n",
      results.allPassed() ? "PASS" : "INCOMPLETE OR FAILED"
  );

  printSeparator();
}

void beginEchoMode() {
  currentMode = Mode::ECHO;

  printSeparator();
  Serial.println(F("ECHO MODE"));
  printSeparator();
  Serial.println(F("Each received byte will be returned immediately."));
  Serial.println(F("Send a line containing only /exit to leave echo mode."));
  Serial.println(F("Local echo should be disabled in Zetta."));
  printSeparator();
}

void processCommand(const char *line) {
  if (strcmp(line, "test") == 0) {
    beginInteractiveTest();
    return;
  }

  if (strcmp(line, "echo") == 0) {
    beginEchoMode();
    return;
  }

  if (strcmp(line, "ascii") == 0) {
    printAsciiTest();
  } else if (strcmp(line, "stress") == 0) {
    printStressTest();
  } else if (strcmp(line, "status") == 0) {
    printStatus();
  } else if (strcmp(line, "clear") == 0) {
    results = TestResults{};
    Serial.println(F("Test results cleared."));
  } else if (strcmp(line, "help") == 0 || line[0] == '\0') {
    printHelp();
  } else if (strcmp(line, "reboot") == 0) {
    Serial.println(F("Restarting ESP32..."));
    Serial.flush();
    delay(100);
    ESP.restart();
  } else {
    Serial.print(F("Unknown command: "));
    printEscaped(line);
    Serial.println();
    Serial.println(F("Enter help for the command list."));
  }

  printPrompt();
}

void processCompletedLine() {
  inputBuffer[inputLength] = '\0';
  ++results.linesReceived;

  switch (currentMode) {
    case Mode::COMMAND:
      processCommand(inputBuffer);
      break;

    case Mode::INTERACTIVE_TEST:
      processInteractiveTestLine(inputBuffer);
      break;

    case Mode::ECHO:
      if (strcmp(inputBuffer, "/exit") == 0) {
        currentMode = Mode::COMMAND;
        Serial.println();
        Serial.println(F("[ECHO MODE ENDED]"));
        printPrompt();
      }
      break;
  }

  inputLength = 0;
}

void eraseDisplayedCharacter() {
  // Move left, overwrite the character, then move left again.
  Serial.print(F("\b \b"));
}

void clearCurrentInputLine() {
  while (inputLength > 0) {
    --inputLength;
    eraseDisplayedCharacter();
  }
}

void processReceivedByte(uint8_t value) {
  ++results.bytesReceived;

  /*
   * Backspace may arrive as:
   *   0x08 = BS
   *   0x7F = DEL
   *
   * Support both because terminal emulators differ.
   */
  if (value == 0x08 || value == 0x7F) {
    if (inputLength > 0) {
      --inputLength;
      inputBuffer[inputLength] = '\0';
      eraseDisplayedCharacter();
    }

    previousWasCR = false;
    return;
  }

  /*
   * Ctrl+U clears the current input line.
   */
  if (value == 0x15) {
    clearCurrentInputLine();
    previousWasCR = false;
    return;
  }

  /*
   * Ctrl+C cancels the current line.
   */
  if (value == 0x03) {
    inputLength = 0;
    inputBuffer[0] = '\0';
    previousWasCR = false;

    Serial.println(F("^C"));

    if (currentMode == Mode::INTERACTIVE_TEST) {
      currentMode = Mode::COMMAND;
      testStage = TestStage::IDLE;
      Serial.println(F("[Interactive test cancelled]"));
    }

    printPrompt();
    return;
  }

  /*
   * Handle Enter. Always display a conventional CRLF on output,
   * irrespective of whether Zetta sends CR, LF, or CRLF.
   */
  if (value == '\r') {
    Serial.print(F("\r\n"));

    lastLineEnding = "CR";
    previousWasCR = true;
    processCompletedLine();
    return;
  }

  if (value == '\n') {
    if (previousWasCR) {
      /*
       * CRLF was already processed when CR arrived.
       */
      lastLineEnding = "CRLF";
      previousWasCR = false;
      return;
    }

    Serial.print(F("\r\n"));

    lastLineEnding = "LF";
    processCompletedLine();
    return;
  }

  previousWasCR = false;

  /*
   * Ignore unsupported control characters rather than placing them
   * into the command buffer.
   */
  if (value < 0x20) {
    return;
  }

  if (inputLength < INPUT_BUFFER_SIZE - 1) {
    inputBuffer[inputLength++] = static_cast<char>(value);
    inputBuffer[inputLength] = '\0';

    /*
     * Remote echo: display the byte after the ESP32 has received it.
     */
    Serial.write(value);
  } else {
    ++results.overflows;

    Serial.println();
    Serial.println(F("[ERROR] Input line exceeded the 255-byte buffer."));

    inputLength = 0;
    inputBuffer[0] = '\0';

    if (currentMode == Mode::COMMAND) {
      printPrompt();
    }
  }
}

void setup() {
  Serial.begin(SERIAL_BAUD);

  /*
   * A short delay makes the initial banner easier to catch after reset.
   * It is not waiting indefinitely for a USB CDC connection.
   */
  delay(500);

  ++bootNumber;

  Serial.println();
  printSeparator();
  Serial.println(F("ZETTA ESP32 SERIAL CONSOLE TEST"));
  printSeparator();
  Serial.printf("Baud rate: %lu\r\n", static_cast<unsigned long>(SERIAL_BAUD));
  Serial.println(F("Format:    8N1"));
  Serial.println(F("Flow:      none"));
  Serial.printf("CPU:       %u MHz\r\n", getCpuFrequencyMhz());
  Serial.printf("Chip:      %s\r\n", ESP.getChipModel());
  Serial.printf("Revision:  %u\r\n", ESP.getChipRevision());
  Serial.printf("Cores:     %u\r\n", ESP.getChipCores());
  Serial.println();
  Serial.println(F("Bidirectional console is ready."));
  Serial.println(F("Enter test to begin, or help for all commands."));
  printSeparator();
  printPrompt();
}

void loop() {
  while (Serial.available() > 0) {
    const int value = Serial.read();

    if (value >= 0) {
      processReceivedByte(static_cast<uint8_t>(value));
    }
  }

  delay(1);
}