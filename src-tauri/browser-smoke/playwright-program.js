page.setDefaultTimeout(5000);
const fixtureBase = new URL(page.url()).origin;
await page.evaluate(base => {
  for(const [id,url] of [['local-frame',base+'/frame'],['remote-frame',base.replace('127.0.0.1','localhost')+'/frame']]){
    const frame=document.createElement('iframe');frame.id=id;frame.src=url;document.body.append(frame);
  }
  const link=document.createElement('a');link.textContent='Popup fixture';link.href=base+'/two';link.target='_blank';document.body.append(link);
},fixtureBase);
await page.frameLocator('#local-frame').getByRole('button',{name:'Frame action'}).click();
await page.screenshot();
await expect(page.frameLocator('#local-frame').locator('#result')).toHaveText('frame clicked');
await page.frameLocator('#remote-frame').getByRole('button',{name:'Frame action'}).click();
await expect(page.frameLocator('#remote-frame').locator('#result')).toHaveText('frame clicked');
const pendingPopup=page.waitForEvent('popup');
await page.getByRole('link',{name:'Popup fixture'}).click();
const popup=await pendingPopup;
await expect(popup).toHaveTitle('storage-retained');
const extra=await context.newPage();await extra.goto(fixtureBase+'/frame');
await expect(extra.getByRole('button',{name:'Frame action'})).toBeVisible();await extra.close();
({frames:page.frames().length,pages:context.pages().length,observation:await tabbit.observe()})
