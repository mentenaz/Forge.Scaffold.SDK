import type { WebPartContext } from "@microsoft/sp-webpart-base";

/** What every service takes: the SPFx context of the web part. */
export interface ServiceContext {
  context: WebPartContext;
}

export const siteOf = ({ context }: ServiceContext): string => context.pageContext.web.absoluteUrl;
