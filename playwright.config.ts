import {defineConfig} from '@playwright/test';
export default defineConfig({testDir:'tests/ui',use:{browserName:'chromium'},reporter:'list'});
