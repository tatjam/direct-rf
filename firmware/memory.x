MEMORY
{
    FLASH     (RX)  : ORIGIN = 0x08000000, LENGTH = 64K
    DTCMRAM   (RWX) : ORIGIN = 0x20000000, LENGTH = 64K
}

/* stm32h7xx-hal uses a PROVIDE that expects RAM symbol to exist */
REGION_ALIAS(RAM, DTCMRAM);