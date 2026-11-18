import { CubismModelSettingJson } from '@framework/cubismmodelsettingjson';
import { CubismDefaultParameterId } from '@framework/cubismdefaultparameterid';
import type { ICubismModelSetting } from '@framework/icubismmodelsetting';
import { BreathParameterData, CubismBreath } from '@framework/effect/cubismbreath';
import { CubismEyeBlink } from '@framework/effect/cubismeyeblink';
import type { CubismIdHandle } from '@framework/id/cubismid';
import { CubismFramework } from '@framework/live2dcubismframework';
import type { CubismMatrix44 } from '@framework/math/cubismmatrix44';
import { CubismUserModel } from '@framework/model/cubismusermodel';
import type { ACubismMotion } from '@framework/motion/acubismmotion';
import { csmMap } from '@framework/type/csmmap';
import { csmVector } from '@framework/type/csmvector';

const PRIORITY_IDLE = 1;

const fetchBuffer = async (url: string): Promise<ArrayBuffer> => {
  const response = await fetch(url);

  if (!response.ok) {
    throw new Error(`${url}: HTTP ${response.status}`);
  }

  return response.arrayBuffer();
};

export class WebModel extends CubismUserModel {
  public ready = false;

  private canvas: HTMLCanvasElement;
  private gl: WebGL2RenderingContext;
  private home: string;
  private setting: ICubismModelSetting;
  private idleGroup: string;
  private motions = new csmMap<string, ACubismMotion>();
  private eyeBlinkIds = new csmVector<CubismIdHandle>();
  private lipSyncIds = new csmVector<CubismIdHandle>();

  private idAngleX = CubismFramework.getIdManager().getId(CubismDefaultParameterId.ParamAngleX);
  private idAngleY = CubismFramework.getIdManager().getId(CubismDefaultParameterId.ParamAngleY);
  private idAngleZ = CubismFramework.getIdManager().getId(CubismDefaultParameterId.ParamAngleZ);
  private idEyeBallX = CubismFramework.getIdManager().getId(CubismDefaultParameterId.ParamEyeBallX);
  private idEyeBallY = CubismFramework.getIdManager().getId(CubismDefaultParameterId.ParamEyeBallY);
  private idBodyAngleX = CubismFramework.getIdManager().getId(CubismDefaultParameterId.ParamBodyAngleX);

  public constructor(canvas: HTMLCanvasElement, gl: WebGL2RenderingContext) {
    super();
    this.canvas = canvas;
    this.gl = gl;
  }

  public async load(home: string, jsonName: string, maskSize = 1024, maskBufferCount = 1): Promise<void> {
    this.home = home;

    const json = await fetchBuffer(home + jsonName);
    this.setting = new CubismModelSettingJson(json, json.byteLength);
    this._modelSetting = this.setting;

    this.loadModel(await fetchBuffer(home + this.setting.getModelFileName()));

    if (this.setting.getPoseFileName() !== '') {
      const pose = await fetchBuffer(home + this.setting.getPoseFileName());
      this.loadPose(pose, pose.byteLength);
    }

    if (this.setting.getPhysicsFileName() !== '') {
      const physics = await fetchBuffer(home + this.setting.getPhysicsFileName());
      this.loadPhysics(physics, physics.byteLength);
    }

    if (this.setting.getUserDataFile() !== '') {
      const userData = await fetchBuffer(home + this.setting.getUserDataFile());
      this.loadUserData(userData, userData.byteLength);
    }

    const layout = new csmMap<string, number>();
    this.setting.getLayoutMap(layout);
    this.getModelMatrix().setupFromLayout(layout);

    if (this.setting.getEyeBlinkParameterCount() > 0) {
      this._eyeBlink = CubismEyeBlink.create(this.setting);
    }

    this._breath = CubismBreath.create();
    const breath: csmVector<BreathParameterData> = new csmVector();
    breath.pushBack(new BreathParameterData(this.idAngleX, 0.0, 15.0, 6.5345, 0.5));
    breath.pushBack(new BreathParameterData(this.idAngleY, 0.0, 8.0, 3.5345, 0.5));
    breath.pushBack(new BreathParameterData(this.idAngleZ, 0.0, 10.0, 5.5345, 0.5));
    breath.pushBack(new BreathParameterData(this.idBodyAngleX, 0.0, 4.0, 15.5345, 0.5));
    breath.pushBack(
      new BreathParameterData(
        CubismFramework.getIdManager().getId(CubismDefaultParameterId.ParamBreath),
        0.5,
        0.5,
        3.2345,
        1
      )
    );
    this._breath.setParameters(breath);

    for (let i = 0; i < this.setting.getEyeBlinkParameterCount(); i++) {
      this.eyeBlinkIds.pushBack(this.setting.getEyeBlinkParameterId(i));
    }
    for (let i = 0; i < this.setting.getLipSyncParameterCount(); i++) {
      this.lipSyncIds.pushBack(this.setting.getLipSyncParameterId(i));
    }

    this.idleGroup = this.setting.getMotionGroupCount() > 0 ? this.setting.getMotionGroupName(0) : '';
    for (let g = 0; g < this.setting.getMotionGroupCount(); g++) {
      const group = this.setting.getMotionGroupName(g);
      if (group === 'Idle') {
        this.idleGroup = group;
      }

      for (let i = 0; i < this.setting.getMotionCount(group); i++) {
        const buffer = await fetchBuffer(home + this.setting.getMotionFileName(group, i));
        const motion = this.loadMotion(buffer, buffer.byteLength, `${group}_${i}`, null, null, this.setting, group, i);

        if (motion) {
          motion.setEffectIds(this.eyeBlinkIds, this.lipSyncIds);
          this.motions.setValue(`${group}_${i}`, motion);
        }
      }
    }

    this._motionManager.stopAllMotions();

    this.createRenderer(maskBufferCount);
    this.getRenderer().setIsPremultipliedAlpha(true);
    this.getRenderer().setClippingMaskBufferSize(maskSize);
    this.getRenderer().startUp(this.gl);

    for (let i = 0; i < this.setting.getTextureCount(); i++) {
      const texture = await this.loadTexture(home + this.setting.getTextureFileName(i));
      this.getRenderer().bindTexture(i, texture);
    }

    this.ready = true;
  }

  private async loadTexture(url: string): Promise<WebGLTexture> {
    const image = await new Promise<HTMLImageElement>((resolve, reject) => {
      const element = new Image();
      element.onload = () => resolve(element);
      element.onerror = () => reject(new Error(`texture failed: ${url}`));
      element.src = url;
    });

    const gl = this.gl;
    const texture = gl.createTexture();

    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, true);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, image);
    gl.generateMipmap(gl.TEXTURE_2D);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    gl.bindTexture(gl.TEXTURE_2D, null);

    image.src = '';

    return texture;
  }

  public update(deltaSeconds: number): void {
    if (!this.ready) {
      return;
    }

    this._dragManager.update(deltaSeconds);
    const dragX = this._dragManager.getX();
    const dragY = this._dragManager.getY();

    let motionUpdated = false;
    this._model.loadParameters();

    if (this._motionManager.isFinished()) {
      this.startMotion(this.idleGroup, Math.floor(Math.random() * this.setting.getMotionCount(this.idleGroup)));
    } else {
      motionUpdated = this._motionManager.updateMotion(this._model, deltaSeconds);
    }

    this._model.saveParameters();

    if (!motionUpdated && this._eyeBlink) {
      this._eyeBlink.updateParameters(this._model, deltaSeconds);
    }

    this._model.addParameterValueById(this.idAngleX, dragX * 30);
    this._model.addParameterValueById(this.idAngleY, dragY * 30);
    this._model.addParameterValueById(this.idAngleZ, dragX * dragY * -30);
    this._model.addParameterValueById(this.idBodyAngleX, dragX * 10);
    this._model.addParameterValueById(this.idEyeBallX, dragX);
    this._model.addParameterValueById(this.idEyeBallY, dragY);

    if (this._breath) {
      this._breath.updateParameters(this._model, deltaSeconds);
    }

    if (this._physics) {
      this._physics.evaluate(this._model, deltaSeconds);
    }

    if (this._pose) {
      this._pose.updateParameters(this._model, deltaSeconds);
    }

    this._model.update();
  }

  private startMotion(group: string, index: number): void {
    if (group === '') {
      return;
    }

    const motion = this.motions.getValue(`${group}_${index}`);

    if (motion) {
      this._motionManager.startMotionPriority(motion, false, PRIORITY_IDLE);
    }
  }

  public draw(projection: CubismMatrix44): void {
    if (!this.ready) {
      return;
    }

    this.getRenderer().setMvpMatrix(projection);
    this.getRenderer().setRenderState(null, [0, 0, this.canvas.width, this.canvas.height]);
    this.getRenderer().drawModel();
  }
}
