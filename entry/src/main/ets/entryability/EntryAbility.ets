import native from 'libgridcraft.so';
import { AbilityConstant, Configuration, UIAbility, Want } from '@kit.AbilityKit';
import { window } from '@kit.ArkUI';
import { BusinessError } from '@kit.BasicServicesKit';
import { publishSystemTheme, systemThemeFromColorMode } from '../SystemTheme';

class SystemThemeInput {
  kind: string = 'system_theme';
  text: string = '';
  constructor(text: string) { this.text = text; }
}

export default class EntryAbility extends UIAbility {
  private syncSystemTheme(colorMode: number | undefined): void {
    const theme = systemThemeFromColorMode(colorMode);
    publishSystemTheme(theme);
    // Push live changes into the native worker; init also carries the first value.
    try {
      native.input(JSON.stringify(new SystemThemeInput(theme)));
    } catch (error) {
      console.info('System theme input unavailable before native init');
    }
  }

  onCreate(want: Want, _launchParam: AbilityConstant.LaunchParam): void {
    this.syncSystemTheme(this.context.config?.colorMode);
    // 冷启动的「打开方式」：want.uri 只出现在 onCreate，必须写进 AppStorage 让页面消费；
    // 只处理 onNewWant 会让首次从文件管理器点开的应用丢掉文件。
    if (want?.uri) {
      AppStorage.setOrCreate('gridcraftOpenWant', want.uri);
    }
    console.info('GridCraft native ability created');
  }

  onNewWant(want: Want): void {
    // 应用已在前台时，系统通过 onNewWant 送来新的文件 URI；页面用 @StorageLink/@Watch 立即消费。
    if (want?.uri) {
      AppStorage.setOrCreate('gridcraftOpenWant', want.uri);
    }
  }

  onConfigurationUpdate(newConfig: Configuration): void {
    this.syncSystemTheme(newConfig.colorMode);
  }

  onForeground(): void {
    native.active(true);
  }

  onBackground(): void {
    native.active(false);
  }

  onWindowStageCreate(windowStage: window.WindowStage): void {
    if (AppStorage.get<string>('gridcraftSystemTheme') === undefined) {
      this.syncSystemTheme(this.context.config?.colorMode);
    }
    windowStage.loadContent('pages/Index', (error: BusinessError) => {
      if (error.code) {
        console.error(`GridCraft page failed: ${error.code} ${error.message}`);
        return;
      }
      this.installCloseInterceptor(windowStage);
    });
  }

  /**
   * 系统标题栏的 X 由窗口管理器直接执行，不经过 egui，所以首版点 X 会直接退出、
   * 不弹 Save Changes?。这里把系统关闭请求拦截下来，转给 Rust 已有的
   * ViewportEvent::Close 流程（native.close → Message::Close → 确认对话框 →
   * notify "close"），由 Index.receive('close') 在用户确认后主动 terminateSelf。
   *
   * resolve(true) = 拦截（窗口保留）；resolve(false) = 放行系统关闭。
   * 依据：SDK @ohos.window.d.ts 的 Window.on('windowWillClose')，@since 15，
   * 仅在点击系统标题栏关闭按钮时触发，且不需要申请权限。
   * 注意：若同时注册 windowStageClose/onPrepareToTerminate 会产生优先级冲突，
   * 因此这里只订阅窗口级的 windowWillClose。
   */
  private installCloseInterceptor(windowStage: window.WindowStage): void {
    try {
      const mainWindow = windowStage.getMainWindowSync();
      mainWindow.on('windowWillClose', async (): Promise<boolean> => {
        console.info('GridCraft window X: intercepted, forwarding to native close validation');
        try {
          native.close();
          // 拦截本次系统关闭；Rust 确认后会 notify "close" → terminateSelf。
          // 用户点 Cancel 时 Rust 不发通知，窗口自然保留。
          return true;
        } catch (error) {
          // native 未就绪时不能把窗口锁死，放行系统关闭作为兜底（保持旧行为）。
          console.error(`GridCraft native close unavailable: ${(error as Error).message}`);
          return false;
        }
      });
      console.info('GridCraft close interceptor registered');
    } catch (error) {
      console.error(`GridCraft close interceptor unavailable: ${(error as Error).message}`);
    }
  }
}
