#include <QApplication>
#include <QContextMenuEvent>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>
#include <QWidget>

class Win : public QMainWindow {
public:
    Win() {
        auto *file = menuBar()->addMenu(QStringLiteral("File"));
        file->addAction(QStringLiteral("Open"), [] { qInfo("ACTION-OPEN-FIRED"); });
        file->addAction(QStringLiteral("Save"), [] { qInfo("ACTION-SAVE-FIRED"); });
        auto *sub = file->addMenu(QStringLiteral("Recent"));
        sub->addAction(QStringLiteral("a.txt"), [] { qInfo("ACTION-RECENT-FIRED"); });
        resize(600, 400);
        setWindowTitle(QStringLiteral("qt-menu-probe"));
    }

protected:
    void contextMenuEvent(QContextMenuEvent *event) override {
        QMenu menu(this);
        menu.addAction(QStringLiteral("CtxOne"), [] { qInfo("ACTION-CTX1-FIRED"); });
        menu.addAction(QStringLiteral("CtxTwo"), [] { qInfo("ACTION-CTX2-FIRED"); });
        menu.exec(event->globalPos());
    }
};

int main(int argc, char **argv) {
    QApplication app(argc, argv);
    Win w;
    w.show();
    qInfo("PROBE-READY");
    return app.exec();
}
